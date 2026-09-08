use std::path::Path;

use crate::vcp_modules::infra::file_extractor::try_extract_text;
use crate::vcp_modules::infra::file_manager::generate_thumbnail;

use super::data::{build_attachment_data, AttachmentDataParts};
use super::{AttachmentData, AttachmentReadGuard};

pub(super) async fn complete_registered_attachment<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: &str,
    original_name: String,
    normalized_mime: String,
    size: u64,
    canonical_path: std::path::PathBuf,
    canonical_path_str: String,
    now: u64,
    gate: &AttachmentReadGuard,
) -> Result<AttachmentData, String> {
    let extracted_text = match extract_registered_text(&canonical_path, &normalized_mime).await {
        Ok(value) => value,
        Err(error) => {
            log::warn!("附件文本提取失败，保留已提交 CAS: {error}");
            None
        }
    };
    let thumbnail_path =
        generate_registered_thumbnail(app_handle, &canonical_path, &normalized_mime, hash).await;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(app_handle)?;
    if let Err(error) = persist_derived_metadata(
        pool,
        hash,
        now,
        extracted_text.as_ref(),
        thumbnail_path.as_ref(),
        &roots,
        gate,
    )
    .await
    {
        log::warn!("附件派生元数据持久化失败，保留已提交 CAS: hash={hash}, error={error}");
    }
    build_attachment_data(AttachmentDataParts {
        hash,
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
    roots: &crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots,
    _gate: &AttachmentReadGuard,
) -> Result<(), String> {
    if extracted_text.is_none() && thumbnail_path.is_none() {
        return Ok(());
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("启动附件派生元数据事务失败: {error}"))?;
    let result = sqlx::query(
        "UPDATE attachments
         SET extracted_text = ?, thumbnail_path = ?, updated_at = ?
         WHERE hash = ?",
    )
    .bind(extracted_text)
    .bind(thumbnail_path)
    .bind(now as i64)
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|error| format!("附件 {hash} 派生元数据更新失败: {error}"))?;
    if result.rows_affected() != 1 {
        return Err(format!(
            "附件 {hash} 派生元数据更新影响 {} 行",
            result.rows_affected()
        ));
    }
    crate::vcp_modules::infra::maintenance_manager::clear_live_attachment_unlink_debts(
        &mut *tx, hash, roots,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| format!("提交附件派生元数据事务失败: {error}"))
}
