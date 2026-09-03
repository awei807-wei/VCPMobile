use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::paths::get_attachments_root_dir;
use super::validation::{
    normalize_attachment_mime, resolve_attachment_cas_file, validate_attachment_cas_path,
};
use crate::vcp_modules::infra::file_extractor::try_extract_text;
use crate::vcp_modules::infra::file_manager::generate_thumbnail;

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

pub(crate) async fn commit_registered_attachment(
    pool: &sqlx::SqlitePool,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    insert_attachment_record(&mut tx, hash, mime_type, size, internal_path, now).await?;
    promote_live_attachment_relations(&mut tx, hash, internal_path).await?;
    tx.commit().await.map_err(|error| error.to_string())
}

async fn insert_attachment_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    hash: &str,
    mime_type: &str,
    size: u64,
    internal_path: &str,
    now: i64,
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
    .bind(hash)
    .bind(mime_type)
    .bind(size as i64)
    .bind(internal_path)
    .bind(now)
    .bind(now)
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
    hash: &str,
    original_name: String,
    size: u64,
    internal_path: &str,
    now: u64,
) -> Result<Option<AttachmentData>, String> {
    if !attachment_record_exists(pool, hash).await? {
        return Ok(None);
    }
    let existing = resolve_attachment_cas_file(app_handle, pool, hash).await?;
    if existing.size_bytes != size {
        return Err(format!("附件 {hash} 的现有 CAS 大小与本次注册不一致"));
    }
    let (thumbnail_path, created_at) = load_existing_attachment_metadata(pool, hash).await?;
    let existing_path = existing.path.to_string_lossy().into_owned();
    let redundant_candidate = validated_redundant_candidate(
        app_handle,
        internal_path,
        hash,
        existing.size_bytes,
        &existing.path,
    )?;
    commit_registered_attachment(
        pool,
        hash,
        &existing.mime_type,
        existing.size_bytes,
        &existing_path,
        now as i64,
    )
    .await?;
    if let Some(candidate) = redundant_candidate {
        if let Err(error) = tokio::fs::remove_file(candidate).await {
            log::warn!("清理重复附件副本失败: {error}");
        }
    }
    Ok(Some(build_attachment_data(AttachmentDataParts {
        hash,
        original_name,
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
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&hash) {
        return Err("非法的 Content-Addressable Storage (CAS) 哈希指纹格式".to_string());
    }
    let now = crate::vcp_modules::infra::utils::now_secs() as u64;
    if let Some(existing) = reuse_existing_attachment(
        app_handle,
        pool,
        &hash,
        original_name.clone(),
        size,
        &internal_path,
        now,
    )
    .await?
    {
        return Ok(existing);
    }
    let normalized_mime = normalize_attachment_mime(&mime_type)?;
    let canonical_path = validate_registered_path(app_handle, &internal_path, &hash, size)?;
    let canonical_path_str = canonical_path.to_string_lossy().into_owned();
    commit_registered_attachment(
        pool,
        &hash,
        &normalized_mime,
        size,
        &canonical_path_str,
        now as i64,
    )
    .await?;
    let extracted_text = extract_registered_text(&canonical_path, &normalized_mime).await?;
    let thumbnail_path =
        generate_registered_thumbnail(app_handle, &canonical_path, &normalized_mime, &hash).await;
    persist_derived_metadata(
        pool,
        &hash,
        now,
        extracted_text.as_ref(),
        thumbnail_path.as_ref(),
    )
    .await;
    build_attachment_data(AttachmentDataParts {
        hash: &hash,
        original_name,
        path: canonical_path,
        internal_path: canonical_path_str,
        mime_type: normalized_mime,
        size,
        created_at: now,
        extracted_text: None,
        thumbnail_path,
    })
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

async fn extract_registered_text(path: &Path, mime_type: &str) -> Result<Option<String>, String> {
    let path = path.to_path_buf();
    let mime_type = mime_type.to_string();
    tokio::task::spawn_blocking(move || try_extract_text(&path, &mime_type))
        .await
        .map_err(|error| format!("Text extraction panicked: {error}"))
}

async fn generate_registered_thumbnail<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    path: &Path,
    mime_type: &str,
    hash: &str,
) -> Option<String> {
    if mime_type.starts_with("image/") {
        generate_thumbnail(app_handle, path, hash).await
    } else {
        None
    }
}

async fn persist_derived_metadata(
    pool: &sqlx::SqlitePool,
    hash: &str,
    now: u64,
    extracted_text: Option<&String>,
    thumbnail_path: Option<&String>,
) {
    if extracted_text.is_none() && thumbnail_path.is_none() {
        return;
    }
    let _ = sqlx::query(
        "UPDATE attachments
         SET extracted_text = ?, thumbnail_path = ?, updated_at = ?
         WHERE hash = ?",
    )
    .bind(extracted_text)
    .bind(thumbnail_path)
    .bind(now as i64)
    .bind(hash)
    .execute(pool)
    .await;
}

struct AttachmentDataParts<'a> {
    hash: &'a str,
    original_name: String,
    path: PathBuf,
    internal_path: String,
    mime_type: String,
    size: u64,
    created_at: u64,
    extracted_text: Option<String>,
    thumbnail_path: Option<String>,
}

fn build_attachment_data(parts: AttachmentDataParts<'_>) -> Result<AttachmentData, String> {
    let internal_file_name = parts
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "附件文件名不是有效 UTF-8".to_string())?
        .to_string();
    Ok(AttachmentData {
        id: format!("attachment_{}", parts.hash),
        name: parts.original_name,
        internal_file_name,
        internal_path: parts.internal_path,
        mime_type: parts.mime_type,
        size: parts.size,
        hash: parts.hash.to_string(),
        created_at: parts.created_at,
        extracted_text: parts.extracted_text,
        thumbnail_path: parts.thumbnail_path,
    })
}
