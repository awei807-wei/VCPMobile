use super::http::{parse_success_response, require_exact_object_keys};
use super::types::{canonical_sha256, MAX_ATTACHMENT_UPLOAD_BYTES};
use crate::vcp_modules::db_manager::DbState;
use sha2::{Digest, Sha256};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

pub(super) async fn upload_attachment<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    hash: &str,
) -> Result<(), String> {
    let hash = canonical_sha256(hash)
        .ok_or_else(|| "Attachment upload requires a valid SHA-256 hash".to_string())?;
    let db = app.state::<DbState>();
    let (mime_type, internal_path) = load_attachment_index(&db.pool, &hash).await?;
    let display_name = load_display_name(&db.pool, &hash).await?;
    let file_path = internal_path.trim_start_matches("file://");
    if file_path.trim().is_empty() {
        return Err(format!("Attachment {hash} has no local file path"));
    }
    let mut file = tokio::fs::File::open(file_path)
        .await
        .map_err(|error| format!("Attachment {hash} read failed: {error}"))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| format!("Attachment {hash} metadata read failed: {error}"))?;
    validate_file_metadata(&hash, &metadata)?;
    verify_file_hash(&mut file, &hash).await?;
    file.seek(std::io::SeekFrom::Start(0))
        .await
        .map_err(|error| format!("Attachment {hash} rewind failed: {error}"))?;

    let url = format!(
        "{http_url}/api/mobile-sync/upload-attachment?hash={hash}&type={}&name={}",
        urlencoding::encode(&mime_type),
        urlencoding::encode(&display_name)
    );
    let response = client
        .post(&url)
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, metadata.len())
        .body(reqwest::Body::wrap_stream(ReaderStream::with_capacity(
            file,
            64 * 1024,
        )))
        .send()
        .await
        .map_err(|error| format!("Attachment {hash} upload failed: {error}"))?;
    let body = parse_success_response(response, "Attachment upload").await?;
    validate_upload_response(&body, &hash)
}

async fn load_attachment_index(
    pool: &sqlx::SqlitePool,
    hash: &str,
) -> Result<(String, String), String> {
    let row = sqlx::query("SELECT mime_type, internal_path FROM attachments WHERE hash = ?")
        .bind(hash)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("Attachment {hash} index query failed: {error}"))?
        .ok_or_else(|| format!("Attachment {hash} is missing from the local index"))?;
    let mime_type: String = row
        .try_get("mime_type")
        .map_err(|error| format!("Attachment {hash} MIME decode failed: {error}"))?;
    let internal_path: String = row
        .try_get("internal_path")
        .map_err(|error| format!("Attachment {hash} path decode failed: {error}"))?;
    Ok((mime_type, internal_path))
}

async fn load_display_name(pool: &sqlx::SqlitePool, hash: &str) -> Result<String, String> {
    let row = sqlx::query(
        "SELECT display_name FROM message_attachments
         WHERE hash = ? AND deleted_at IS NULL LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("Attachment {hash} display name query failed: {error}"))?
    .ok_or_else(|| format!("Attachment {hash} has no live message relation"))?;
    let name: String = row
        .try_get("display_name")
        .map_err(|error| format!("Attachment {hash} display name decode failed: {error}"))?;
    if name.is_empty() {
        return Err(format!("Attachment {hash} has an empty display name"));
    }
    Ok(name)
}

fn validate_file_metadata(hash: &str, metadata: &std::fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() {
        return Err(format!("Attachment {hash} path is not a regular file"));
    }
    if metadata.len() > MAX_ATTACHMENT_UPLOAD_BYTES {
        return Err(format!(
            "Attachment {hash} exceeds the 512 MiB upload limit"
        ));
    }
    Ok(())
}

async fn verify_file_hash(file: &mut tokio::fs::File, expected: &str) -> Result<(), String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| format!("Attachment {expected} hash read failed: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != expected {
        return Err(format!(
            "Attachment {expected} content hash mismatch (actual {actual_hash})"
        ));
    }
    Ok(())
}

pub(super) fn validate_upload_response(
    value: &serde_json::Value,
    expected: &str,
) -> Result<(), String> {
    let object = require_exact_object_keys(value, &["success", "hash"], "Attachment upload")?;
    let response_hash = object
        .get("hash")
        .and_then(serde_json::Value::as_str)
        .and_then(canonical_sha256)
        .ok_or_else(|| "Attachment upload response requires a valid hash".to_string())?;
    if object.get("success").and_then(serde_json::Value::as_bool) != Some(true)
        || response_hash != expected
    {
        return Err(format!(
            "Attachment upload response hash mismatch: expected {expected}, got {response_hash}"
        ));
    }
    log::debug!("[PushExecutor] Attachment uploaded: {}", expected);
    Ok(())
}
