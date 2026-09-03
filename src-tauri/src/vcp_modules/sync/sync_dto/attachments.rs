use crate::vcp_modules::chat_manager::Attachment;
use crate::vcp_modules::sync::sync_types::{
    deserialize_optional_safe_u64, deserialize_safe_u64, serialize_optional_safe_u64,
    serialize_safe_u64, validate_safe_non_negative_u64,
};
use serde::{Deserialize, Serialize};

/// Attachment metadata that is safe to cross the sync boundary.
///
/// Local paths, thumbnail paths, status, and the legacy `_fileManagerData`
/// object are deliberately not represented here. The content-addressed hash
/// is the only attachment identity that the peer can use to request the
/// corresponding CAS blob.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentSyncDTO {
    pub r#type: String,
    pub name: String,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub size: u64,
    #[serde(deserialize_with = "deserialize_sha256")]
    pub hash: String,
    #[serde(rename = "attachmentOrder", skip_serializing_if = "Option::is_none")]
    pub attachment_order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(
        deserialize_with = "deserialize_optional_safe_u64",
        serialize_with = "serialize_optional_safe_u64"
    )]
    pub created_at: Option<u64>,
    /// Transitional Rust-only field for old executor constructors. It is
    /// never serialized; new Wire 1.4 code must leave it as `None`.
    #[serde(skip)]
    #[allow(dead_code)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachmentWireDTO {
    r#type: String,
    name: String,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    size: u64,
    #[serde(deserialize_with = "deserialize_sha256")]
    hash: String,
    #[serde(rename = "attachmentOrder", default)]
    attachment_order: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extracted_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_frames: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(
        deserialize_with = "deserialize_optional_safe_u64",
        serialize_with = "serialize_optional_safe_u64"
    )]
    created_at: Option<u64>,
}

impl<'de> Deserialize<'de> for AttachmentSyncDTO {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AttachmentWireDTO::deserialize(deserializer)?;
        Ok(Self {
            r#type: wire.r#type,
            name: wire.name,
            size: wire.size,
            hash: wire.hash,
            attachment_order: validate_attachment_order(wire.attachment_order)
                .map_err(serde::de::Error::custom)?,
            extracted_text: wire.extracted_text,
            image_frames: wire.image_frames,
            created_at: wire.created_at,
            status: None,
        })
    }
}

impl TryFrom<&Attachment> for AttachmentSyncDTO {
    type Error = String;

    fn try_from(att: &Attachment) -> Result<Self, Self::Error> {
        let hash = att
            .hash
            .as_deref()
            .map(str::to_ascii_lowercase)
            .filter(|hash| crate::vcp_modules::infra::utils::is_valid_cas_hash(hash))
            .ok_or_else(|| {
                format!(
                    "Attachment {} requires a valid SHA-256 content hash",
                    att.name
                )
            })?;
        validate_safe_non_negative_u64(att.size, "Attachment size")?;
        if let Some(created_at) = att.created_at {
            validate_safe_non_negative_u64(created_at, "Attachment createdAt")?;
        }
        Ok(Self {
            r#type: att.r#type.clone(),
            name: att.name.clone(),
            size: att.size,
            hash,
            attachment_order: validate_attachment_order(att.attachment_order)?,
            extracted_text: att.extracted_text.clone(),
            image_frames: att.image_frames.clone(),
            created_at: att.created_at,
            status: None,
        })
    }
}

fn deserialize_sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let normalized = value.to_ascii_lowercase();
    if crate::vcp_modules::infra::utils::is_valid_cas_hash(&normalized) {
        Ok(normalized)
    } else {
        Err(serde::de::Error::custom(
            "hash must be a 64-character SHA-256 hex string",
        ))
    }
}

pub(super) fn validate_attachment_order(order: Option<i32>) -> Result<Option<i32>, String> {
    match order {
        Some(value) if value < 0 => Err("attachmentOrder must be non-negative".to_string()),
        other => Ok(other),
    }
}
