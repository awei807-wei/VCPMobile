use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::paths::get_attachments_root_dir;
#[path = "data.rs"]
mod data;
use super::validation::{
    normalize_attachment_mime, resolve_attachment_cas_file, validate_attachment_cas_path,
};

use super::{attachment_gc_gate, AttachmentReadGuard};
use data::{build_attachment_data, AttachmentDataParts};

#[path = "registration_finalize.rs"]
mod registration_finalize;

/// 附件元数据结构
/// 对齐 @/plans/Rust文件数据管理重构详细规划.md 中的 2.1 节
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentData {
    pub id: String,
    pub name: String,
    pub internal_file_name: String,
    pub internal_path: String,
    #[serde(rename = "type")]
    pub mime_type: String, // 对应 JS 端的 type
    pub size: u64,
    pub hash: String,
    pub created_at: u64,
    pub extracted_text: Option<String>,
    pub thumbnail_path: Option<String>,
}

pub(crate) struct AttachmentRegistrationInput {
    pub(crate) hash: String,
    pub(crate) original_name: String,
    pub(crate) mime_type: String,
    pub(crate) size: u64,
    pub(crate) internal_path: String,
}

impl AttachmentRegistrationInput {
    pub(crate) fn new(
        hash: String,
        original_name: String,
        mime_type: String,
        size: u64,
        internal_path: String,
    ) -> Self {
        Self {
            hash,
            original_name,
            mime_type,
            size,
            internal_path,
        }
    }
}

#[derive(Clone, Copy)]
struct RegisteredAttachmentRecord<'a> {
    hash: &'a str,
    mime_type: &'a str,
    size: u64,
    internal_path: &'a str,
    now: i64,
}

pub(crate) async fn commit_registered_attachment(
    pool: &sqlx::SqlitePool,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
) -> Result<(), String> {
    let _gate = attachment_gc_gate().read().await;
    commit_registered_attachment_unlocked_with_roots(
        pool,
        RegisteredAttachmentRecord {
            hash,
            mime_type,
            size,
            internal_path,
            now,
        },
        None,
        &_gate,
    )
    .await
}

pub(crate) async fn commit_registered_attachment_unlocked(
    pool: &sqlx::SqlitePool,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
    _gate: &AttachmentReadGuard,
) -> Result<(), String> {
    commit_registered_attachment_unlocked_with_roots(
        pool,
        RegisteredAttachmentRecord {
            hash,
            mime_type,
            size,
            internal_path,
            now,
        },
        None,
        _gate,
    )
    .await
}

async fn commit_registered_attachment_unlocked_with_roots(
    pool: &sqlx::SqlitePool,
    record: RegisteredAttachmentRecord<'_>,
    roots: Option<&crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots>,
    _gate: &AttachmentReadGuard,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    insert_attachment_record(&mut tx, record).await?;
    promote_live_attachment_relations(&mut tx, record.hash, record.internal_path).await?;
    if let Some(roots) = roots {
        crate::vcp_modules::infra::maintenance_manager::clear_live_attachment_unlink_debts(
            &mut tx,
            record.hash,
            roots,
        )
        .await?;
    }
    tx.commit().await.map_err(|error| error.to_string())
}

async fn insert_attachment_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    record: RegisteredAttachmentRecord<'_>,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO attachments (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
            mime_type = excluded.mime_type,
            size = excluded.size,
            internal_path = excluded.internal_path,
            updated_at = excluded.updated_at",
    )
    .bind(record.hash)
    .bind(record.mime_type)
    .bind(record.size as i64)
    .bind(record.internal_path)
    .bind(record.now)
    .bind(record.now)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn promote_live_attachment_relations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    hash: &str,
    internal_path: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE message_attachments
         SET status = 'ready', src = ?
         WHERE hash = ? AND status = 'desktop_only' AND deleted_at IS NULL
           AND EXISTS (
             SELECT 1 FROM messages m
             WHERE m.owner_type = message_attachments.owner_type
               AND m.owner_id = message_attachments.owner_id
               AND m.topic_id = message_attachments.topic_id
               AND m.msg_id = message_attachments.msg_id
               AND m.deleted_at IS NULL
           )
           AND EXISTS (
             SELECT 1 FROM topics t
             WHERE t.owner_type = message_attachments.owner_type
               AND t.owner_id = message_attachments.owner_id
               AND t.topic_id = message_attachments.topic_id
               AND t.deleted_at IS NULL
               AND (
                 (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a
                   WHERE a.agent_id = t.owner_id AND a.deleted_at IS NULL
                 ))
                 OR
                 (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g
                   WHERE g.group_id = t.owner_id AND g.deleted_at IS NULL
                 ))
               )
           )",
    )
    .bind(format!("file://{internal_path}"))
    .bind(hash)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn reuse_existing_attachment<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    input: &AttachmentRegistrationInput,
    now: u64,
    roots: &crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    _gate: &AttachmentReadGuard,
) -> Result<Option<AttachmentData>, String> {
    if !attachment_record_exists(pool, &input.hash).await? {
        return Ok(None);
    }
    let existing = resolve_attachment_cas_file(app_handle, pool, &input.hash).await?;
    if existing.size_bytes != input.size {
        return Err(format!(
            "附件 {} 的现有 CAS 大小与本次注册不一致",
            input.hash
        ));
    }
    let (thumbnail_path, created_at) = load_existing_attachment_metadata(pool, &input.hash).await?;
    let existing_path = existing.path.to_string_lossy().into_owned();
    let redundant_candidate = validated_redundant_candidate(
        app_handle,
        &input.internal_path,
        &input.hash,
        existing.size_bytes,
        &existing.path,
    )?;
    commit_registered_attachment_unlocked_with_roots(
        pool,
        RegisteredAttachmentRecord {
            hash: &input.hash,
            mime_type: &existing.mime_type,
            size: existing.size_bytes,
            internal_path: &existing_path,
            now: now as i64,
        },
        Some(roots),
        _gate,
    )
    .await?;
    if let Some(candidate) = redundant_candidate {
        if let Err(error) = tokio::fs::remove_file(candidate).await {
            log::warn!("清理重复附件副本失败: {error}");
        }
    }
    Ok(Some(build_attachment_data(AttachmentDataParts {
        hash: &input.hash,
        original_name: input.original_name.clone(),
        path: existing.path,
        internal_path: existing_path,
        mime_type: existing.mime_type,
        size: existing.size_bytes,
        created_at,
        extracted_text: None,
        thumbnail_path,
    })?))
}

async fn attachment_record_exists(pool: &sqlx::SqlitePool, hash: &str) -> Result<bool, String> {
    let exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments WHERE hash = ?)")
        .bind(hash)
        .fetch_one(pool)
        .await
        .map_err(|error| format!("读取现有附件记录失败: {error}"))?;
    Ok(exists != 0)
}

async fn load_existing_attachment_metadata(
    pool: &sqlx::SqlitePool,
    hash: &str,
) -> Result<(Option<String>, u64), String> {
    let (thumbnail_path, created_at): (Option<String>, i64) =
        sqlx::query_as("SELECT thumbnail_path, created_at FROM attachments WHERE hash = ?")
            .bind(hash)
            .fetch_one(pool)
            .await
            .map_err(|error| format!("读取现有附件元数据失败: {error}"))?;
    let created_at = u64::try_from(created_at).map_err(|_| "现有附件创建时间无效".to_string())?;
    Ok((thumbnail_path, created_at))
}

fn validated_redundant_candidate<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    internal_path: &str,
    hash: &str,
    size: u64,
    existing_path: &Path,
) -> Result<Option<PathBuf>, String> {
    let candidate = Path::new(internal_path.trim_start_matches("file://"));
    if !candidate.exists() {
        return Ok(None);
    }
    let attachments_root = get_attachments_root_dir(app_handle)?;
    let candidate = validate_attachment_cas_path(&attachments_root, candidate, hash, size)?;
    Ok((candidate != existing_path).then_some(candidate))
}

/// 将文件元数据注册到数据库并触发后处理 (缩略图、文本提取)
pub async fn register_attachment_internal<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: String,
    original_name: String,
    mime_type: String,
    size: u64,
    internal_path: String,
) -> Result<AttachmentData, String> {
    let _gate = attachment_gc_gate().read().await;
    register_attachment_internal_unlocked(
        app_handle,
        pool,
        AttachmentRegistrationInput::new(hash, original_name, mime_type, size, internal_path),
        &_gate,
    )
    .await
}

/// 在调用方已经持有附件 GC 读锁时执行注册，避免读锁嵌套遇到等待中的 GC 写锁。
pub(crate) async fn register_attachment_internal_unlocked<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    input: AttachmentRegistrationInput,
    _gate: &AttachmentReadGuard,
) -> Result<AttachmentData, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&input.hash) {
        return Err("非法的 Content-Addressable Storage (CAS) 哈希指纹格式".to_string());
    }
    let now = crate::vcp_modules::infra::utils::now_secs() as u64;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(app_handle)?;
    if let Some(existing) =
        reuse_existing_attachment(app_handle, pool, &input, now, &roots, _gate).await?
    {
        return Ok(existing);
    }
    let AttachmentRegistrationInput {
        hash,
        original_name,
        mime_type,
        size,
        internal_path,
    } = input;
    let normalized_mime = normalize_attachment_mime(&mime_type)?;
    let canonical_path = validate_registered_path(app_handle, &internal_path, &hash, size)?;
    let canonical_path_str = canonical_path.to_string_lossy().into_owned();
    commit_registered_attachment_unlocked_with_roots(
        pool,
        RegisteredAttachmentRecord {
            hash: &hash,
            mime_type: &normalized_mime,
            size,
            internal_path: &canonical_path_str,
            now: now as i64,
        },
        Some(&roots),
        _gate,
    )
    .await?;
    registration_finalize::complete_registered_attachment(
        app_handle,
        pool,
        AttachmentDataParts {
            hash: &hash,
            original_name,
            path: canonical_path,
            internal_path: canonical_path_str,
            mime_type: normalized_mime,
            size,
            created_at: now,
            extracted_text: None,
            thumbnail_path: None,
        },
        _gate,
    )
    .await
}

fn validate_registered_path<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    internal_path: &str,
    hash: &str,
    size: u64,
) -> Result<PathBuf, String> {
    let path = PathBuf::from(internal_path.trim_start_matches("file://"));
    let root = get_attachments_root_dir(app_handle)?;
    validate_attachment_cas_path(&root, &path, hash, size)
}
