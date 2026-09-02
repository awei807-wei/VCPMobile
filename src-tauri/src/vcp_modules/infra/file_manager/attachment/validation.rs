use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tauri::AppHandle;

pub(crate) const STAGED_FILE_MAX_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) fn store_file_semaphore() -> Arc<tokio::sync::Semaphore> {
    static SEMAPHORE: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
}

pub(crate) fn safe_storage_extension(original_name: &str) -> Option<&str> {
    let extension = Path::new(original_name)
        .extension()
        .and_then(|extension| extension.to_str())?;
    if extension.is_empty()
        || extension.len() > 16
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        None
    } else {
        Some(extension)
    }
}

pub(crate) fn canonical_file_within_root(
    root: &Path,
    file: &Path,
    label: &str,
) -> Result<PathBuf, String> {
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("{} staging 根目录不可用: {}", label, error))?;
    let canonical_file = fs::canonicalize(file)
        .map_err(|error| format!("{} staging 文件不可用: {}", label, error))?;
    if !canonical_file.starts_with(&canonical_root) || !canonical_file.is_file() {
        return Err(format!("非法的 {} staging 文件路径", label));
    }
    Ok(canonical_file)
}

pub(crate) fn verify_expected_hash(
    expected_hash: Option<&str>,
    actual_hash: &str,
) -> Result<(), String> {
    if let Some(expected_hash) = expected_hash {
        if expected_hash != actual_hash {
            return Err("Native staging hash 与 Rust 重算结果不一致".to_string());
        }
    }
    Ok(())
}

pub(crate) fn check_existing_cas_size(path: &Path, expected_size: u64) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.file_type().is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    Ok(())
}

pub(crate) async fn check_existing_cas_size_async(
    path: &Path,
    expected_size: u64,
) -> Result<(), String> {
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| format!("CAS 文件元数据读取失败: {}", error))?;
    if !metadata.file_type().is_file() || metadata.len() != expected_size {
        return Err("已存在的 CAS 文件大小不匹配，拒绝复用".to_string());
    }
    Ok(())
}

/// Rust-only facts about a validated CAS attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentCasFile {
    pub path: PathBuf,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

pub(crate) async fn resolve_attachment_cas_file<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: &str,
) -> Result<AttachmentCasFile, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err("invalid attachment CAS hash".to_string());
    }

    let record = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT mime_type, size, internal_path FROM attachments WHERE hash = ? LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("cannot read attachment CAS metadata: {error}"))?
    .ok_or_else(|| "attachment is not present in the local CAS catalog".to_string())?;
    let size_bytes = u64::try_from(record.1)
        .map_err(|_| "attachment CAS size metadata is invalid".to_string())?;
    let mime_type = normalize_attachment_mime(&record.0)?;
    let internal_path = record.2.trim_start_matches("file://");
    let attachments_root = super::paths::get_attachments_root_dir(app_handle)?;
    let path = validate_attachment_cas_path(
        &attachments_root,
        Path::new(internal_path),
        hash,
        size_bytes,
    )?;

    Ok(AttachmentCasFile {
        path,
        mime_type,
        size_bytes,
        sha256: hash.to_string(),
    })
}

pub(crate) fn normalize_attachment_mime(value: &str) -> Result<String, String> {
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let Some((kind, subtype)) = mime.split_once('/') else {
        return Err("attachment CAS MIME metadata is invalid".to_string());
    };
    let valid_token = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                    )
            })
    };
    if mime.len() > 128 || !valid_token(kind) || !valid_token(subtype) {
        return Err("attachment CAS MIME metadata is invalid".to_string());
    }
    Ok(mime)
}

pub(crate) fn validate_attachment_cas_path(
    attachments_root: &Path,
    candidate: &Path,
    hash: &str,
    expected_size: u64,
) -> Result<PathBuf, String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return Err("attachment CAS hash is invalid".to_string());
    }
    let source_metadata = fs::symlink_metadata(candidate)
        .map_err(|error| format!("cannot inspect attachment CAS file: {error}"))?;
    if !source_metadata.file_type().is_file() || source_metadata.len() != expected_size {
        return Err("attachment CAS file is not the expected real regular file".to_string());
    }
    let canonical_root = fs::canonicalize(attachments_root)
        .map_err(|error| format!("cannot resolve attachment CAS root: {error}"))?;
    let canonical = fs::canonicalize(candidate)
        .map_err(|error| format!("cannot resolve attachment CAS file: {error}"))?;
    if canonical.parent() != Some(canonical_root.as_path()) {
        return Err("attachment CAS file escaped its fixed root".to_string());
    }
    let stem = canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "attachment CAS file name is invalid".to_string())?;
    if stem != hash {
        return Err("attachment CAS file name does not match its hash".to_string());
    }
    Ok(canonical)
}
