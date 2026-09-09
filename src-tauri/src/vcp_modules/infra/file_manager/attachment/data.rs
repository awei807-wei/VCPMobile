use super::AttachmentData;
use std::path::PathBuf;

pub(super) struct AttachmentDataParts<'a> {
    pub(super) hash: &'a str,
    pub(super) original_name: String,
    pub(super) path: PathBuf,
    pub(super) internal_path: String,
    pub(super) mime_type: String,
    pub(super) size: u64,
    pub(super) created_at: u64,
    pub(super) extracted_text: Option<String>,
    pub(super) thumbnail_path: Option<String>,
}

pub(super) fn build_attachment_data(
    parts: AttachmentDataParts<'_>,
) -> Result<AttachmentData, String> {
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
