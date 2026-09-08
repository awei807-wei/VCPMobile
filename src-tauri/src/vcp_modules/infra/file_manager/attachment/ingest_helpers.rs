use crate::vcp_modules::db_manager::DbState;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

use super::paths::{get_attachments_root_dir, get_thumbnails_root_dir};
use super::registration::AttachmentData;
use super::validation::{
    canonical_file_within_root, check_existing_cas_size, check_existing_cas_size_async,
    safe_storage_extension, verify_expected_hash, STAGED_FILE_MAX_BYTES,
};
use crate::vcp_modules::infra::file_manager::get_refined_mime_type;

pub(crate) struct PreparedStaging {
    pub source_path: PathBuf,
    pub thumbnail_path: Option<PathBuf>,
    pub size: u64,
    pub hash: String,
}

pub(crate) struct PromotedFile {
    pub path: PathBuf,
    pub placement: Placement,
}

#[derive(Clone, Copy)]
pub(crate) enum Placement {
    Existing,
    Renamed,
    Copied,
}

pub(crate) fn validate_memory_upload(size: usize) -> Result<(), String> {
    if size > 100 * 1024 * 1024 {
        Err("文件过大，请使用高速链路上传 (Limit: 100MB)".to_string())
    } else {
        Ok(())
    }
}

pub(crate) fn memory_cas_path(attachments_dir: &Path, hash: &str, original_name: &str) -> PathBuf {
    let file_name = safe_storage_extension(original_name)
        .map(|extension| format!("{hash}.{extension}"))
        .unwrap_or_else(|| hash.to_string());
    attachments_dir.join(file_name)
}

pub(crate) fn write_memory_cas(
    attachments_dir: &Path,
    hash: &str,
    original_name: &str,
    file_bytes: &[u8],
) -> Result<PathBuf, String> {
    if !attachments_dir.exists() {
        fs::create_dir_all(attachments_dir).map_err(|error| error.to_string())?;
    }
    let destination = memory_cas_path(attachments_dir, hash, original_name);
    if destination.exists() {
        check_existing_cas_size(&destination, file_bytes.len() as u64)?;
        return Ok(destination);
    }

    let temporary = attachments_dir.join(format!(".ingest-{hash}-{}.tmp", uuid::Uuid::new_v4()));
    if let Err(error) = fs::write(&temporary, file_bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        if !destination.exists() {
            return Err(error.to_string());
        }
        check_existing_cas_size(&destination, file_bytes.len() as u64)?;
    }
    Ok(destination)
}

pub(crate) async fn resolve_staging_paths(
    app_handle: &AppHandle,
    local_path: &str,
    thumbnail_path: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let uploads_root = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?
        .join("uploads");
    tokio::fs::create_dir_all(&uploads_root)
        .await
        .map_err(|error| format!("无法创建上传 staging 目录: {error}"))?;
    let canonical_uploads_root = fs::canonicalize(&uploads_root)
        .map_err(|error| format!("上传 staging 根目录规范化失败: {error}"))?;
    let source_path = canonical_file_within_root(&uploads_root, Path::new(local_path), "附件")?;
    if source_path.parent() != Some(canonical_uploads_root.as_path()) {
        return Err("附件必须是 uploads staging 根目录的直接文件".to_string());
    }
    let thumbnail = match thumbnail_path {
        Some(path) => Some(resolve_staged_thumbnail(&uploads_root, path).await?),
        None => None,
    };
    Ok((source_path, thumbnail))
}

async fn resolve_staged_thumbnail(uploads_root: &Path, path: &str) -> Result<PathBuf, String> {
    let thumbnail_root = uploads_root.join("thumbnails");
    tokio::fs::create_dir_all(&thumbnail_root)
        .await
        .map_err(|error| format!("无法创建缩略图 staging 目录: {error}"))?;
    let canonical_root = fs::canonicalize(&thumbnail_root)
        .map_err(|error| format!("缩略图 staging 根目录规范化失败: {error}"))?;
    let staged = canonical_file_within_root(&thumbnail_root, Path::new(path), "缩略图")?;
    if staged.parent() != Some(canonical_root.as_path()) {
        return Err("缩略图必须是专用 staging 根目录的直接文件".to_string());
    }
    Ok(staged)
}

pub(crate) async fn prepare_staged_upload(
    app_handle: &AppHandle,
    local_path: &str,
    thumbnail_path: Option<&str>,
    expected_hash: Option<&str>,
    stable_id: Option<&str>,
) -> Result<PreparedStaging, String> {
    validate_expected_hash_format(expected_hash)?;
    let (source_path, thumbnail_path) =
        resolve_staging_paths(app_handle, local_path, thumbnail_path).await?;
    let size = tokio::fs::metadata(&source_path)
        .await
        .map_err(|error| format!("无法读取源文件元数据: {error}"))?
        .len();
    if size > STAGED_FILE_MAX_BYTES {
        return Err("staging 文件过大 (Limit: 512MB)".to_string());
    }
    let hash = hash_staged_file(app_handle, &source_path, size, stable_id).await?;
    verify_expected_hash(expected_hash, &hash)?;
    Ok(PreparedStaging {
        source_path,
        thumbnail_path,
        size,
        hash,
    })
}

fn validate_expected_hash_format(expected_hash: Option<&str>) -> Result<(), String> {
    if expected_hash.is_some_and(|hash| !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
    {
        return Err("非法的 Content-Addressable Storage (CAS) 哈希指纹格式".to_string());
    }
    Ok(())
}

async fn hash_staged_file(
    app_handle: &AppHandle,
    source_path: &Path,
    size: u64,
    stable_id: Option<&str>,
) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(source_path)
        .await
        .map_err(|error| format!("无法打开源文件: {error}"))?;
    let mut hasher = Sha256::new();
    let mut hashed_bytes = 0u64;
    let mut buffer = [0u8; 65536];
    let mut last_emit_time = std::time::Instant::now();
    loop {
        let bytes_read = file
            .read(&mut buffer)
            .await
            .map_err(|error| format!("读取源文件失败: {error}"))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        hashed_bytes += bytes_read as u64;
        emit_hash_progress(
            app_handle,
            stable_id,
            size,
            hashed_bytes,
            &mut last_emit_time,
        );
    }
    Ok(hex::encode(hasher.finalize()))
}

fn emit_hash_progress(
    app_handle: &AppHandle,
    stable_id: Option<&str>,
    size: u64,
    hashed_bytes: u64,
    last_emit_time: &mut std::time::Instant,
) {
    let Some(stable_id) = stable_id else {
        return;
    };
    let now = std::time::Instant::now();
    if now.duration_since(*last_emit_time).as_millis() <= 200 {
        return;
    }
    *last_emit_time = now;
    let percent = if size > 0 {
        (hashed_bytes as f64 / size as f64 * 100.0) as u32
    } else {
        0
    };
    emit_progress(app_handle, Some(stable_id), 50 + (percent * 40 / 100));
}

pub(crate) async fn promote_staged_file(
    app_handle: &AppHandle,
    staging: &PreparedStaging,
    original_name: &str,
    stable_id: Option<&str>,
) -> Result<PromotedFile, String> {
    let attachments_dir = get_attachments_root_dir(app_handle)?;
    tokio::fs::create_dir_all(&attachments_dir)
        .await
        .map_err(|error| error.to_string())?;
    let destination = memory_cas_path(&attachments_dir, &staging.hash, original_name);
    if destination.exists() {
        check_existing_cas_size_async(&destination, staging.size).await?;
        emit_progress(app_handle, stable_id, 99);
        return Ok(PromotedFile {
            path: destination,
            placement: Placement::Existing,
        });
    }

    emit_progress(app_handle, stable_id, 90);
    let placement = if tokio::fs::rename(&staging.source_path, &destination)
        .await
        .is_ok()
    {
        Placement::Renamed
    } else {
        copy_staging_to_cas(&staging.source_path, &destination, &staging.hash).await?
    };
    emit_progress(app_handle, stable_id, 99);
    Ok(PromotedFile {
        path: destination,
        placement,
    })
}

async fn copy_staging_to_cas(
    source: &Path,
    destination: &Path,
    hash: &str,
) -> Result<Placement, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "附件目标缺少父目录".to_string())?;
    let temporary = parent.join(format!(".ingest-{hash}-{}.tmp", uuid::Uuid::new_v4()));
    if let Err(error) = tokio::fs::copy(source, &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(format!("复制文件到正式目录失败: {error}"));
    }
    match tokio::fs::rename(&temporary, destination).await {
        Ok(()) => Ok(Placement::Copied),
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            if !destination.exists() {
                return Err(format!("提交文件到正式目录失败: {error}"));
            }
            Ok(Placement::Existing)
        }
    }
}

pub(crate) fn emit_progress(app_handle: &AppHandle, stable_id: Option<&str>, progress: u32) {
    let Some(stable_id) = stable_id else {
        return;
    };
    app_handle
        .emit(
            "vcp-file-register-progress",
            serde_json::json!({
                "progress": progress,
                "stableId": stable_id,
            }),
        )
        .ok();
}

pub(crate) async fn register_promoted_file(
    app_handle: &AppHandle,
    db_state: &DbState,
    staging: &PreparedStaging,
    promoted: &PromotedFile,
    original_name: String,
    mime_type: Option<String>,
    gate: &super::AttachmentReadGuard,
) -> Result<AttachmentData, String> {
    let initial_mime = mime_type.unwrap_or_else(|| "application/octet-stream".to_string());
    let refined_mime = get_refined_mime_type(&promoted.path, &original_name, &initial_mime);
    super::registration::register_attachment_internal_unlocked(
        app_handle,
        &db_state.pool,
        staging.hash.clone(),
        original_name,
        refined_mime,
        staging.size,
        promoted.path.to_string_lossy().into_owned(),
        gate,
    )
    .await
}

pub(crate) async fn process_staged_thumbnail(
    app_handle: &AppHandle,
    db_state: &DbState,
    source: &Path,
    hash: &str,
    created_at: u64,
    _gate: &super::AttachmentReadGuard,
) -> Result<String, String> {
    let thumbs_dir = get_thumbnails_root_dir(app_handle)?;
    tokio::fs::create_dir_all(&thumbs_dir)
        .await
        .map_err(|error| error.to_string())?;
    let destination = thumbs_dir.join(format!("{hash}_thumb.webp"));
    let destination_str = destination
        .to_str()
        .ok_or("无效的缩略图目标路径字符")?
        .to_string();
    if !destination.exists() {
        copy_thumbnail_to_cas(source, &destination, hash).await?;
    }
    let mut tx = db_state
        .pool
        .begin()
        .await
        .map_err(|error| format!("启动附件缩略图元数据事务失败: {error}"))?;
    sqlx::query("UPDATE attachments SET thumbnail_path = ?, updated_at = ? WHERE hash = ?")
        .bind(&destination_str)
        .bind(created_at as i64)
        .bind(hash)
        .execute(&mut *tx)
        .await
        .map_err(|error| format!("更新附件缩略图元数据失败: {error}"))?;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(app_handle)?;
    crate::vcp_modules::infra::maintenance_manager::clear_live_attachment_unlink_debts(
        &mut *tx, hash, &roots,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| format!("提交附件缩略图元数据事务失败: {error}"))?;
    if let Err(error) = tokio::fs::remove_file(source).await {
        log::warn!("附件缩略图已注册，但清理 staging 文件失败: {error}");
    }
    Ok(destination_str)
}

async fn copy_thumbnail_to_cas(
    source: &Path,
    destination: &Path,
    hash: &str,
) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "缩略图目标缺少父目录".to_string())?;
    let temporary = parent.join(format!(".thumb-{hash}-{}.tmp", uuid::Uuid::new_v4()));
    if let Err(error) = tokio::fs::copy(source, &temporary).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(format!("复制缩略图到正式目录失败: {error}"));
    }
    if let Err(error) = tokio::fs::rename(&temporary, destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !destination.exists() {
            return Err(format!("提交缩略图到正式目录失败: {error}"));
        }
    }
    Ok(())
}

pub(crate) async fn cleanup_staged_source(source: &Path, placement: Placement) {
    if matches!(placement, Placement::Existing | Placement::Copied) {
        if let Err(error) = tokio::fs::remove_file(source).await {
            log::warn!("附件已注册，但清理 staging 文件失败: {error}");
        }
    }
}
