use super::AttachmentSyncDTO;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync::sync_types::{
    deserialize_safe_u64, serialize_safe_u64, validate_safe_non_negative_u64,
};
use serde::{Deserialize, Serialize};

/// The one canonical message contract used for both push and pull.
///
/// This is intentionally a strict wire DTO. Local-only message fields such
/// as `avatarColor`, render blocks, attachment paths/status, and tombstone
/// bookkeeping are not part of the contract and therefore fail closed when
/// received instead of silently crossing the boundary.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSyncDTO {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub timestamp: u64,
    #[serde(rename = "updatedAt")]
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub updated_at: u64,
    #[serde(rename = "agentId", default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    #[serde(
        rename = "isGroupMessage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub is_group_message: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentSyncDTO>>,
    #[serde(rename = "contentHash", deserialize_with = "deserialize_content_hash")]
    pub content_hash: String,
}

impl MessageSyncDTO {
    /// Build a canonical wire message with an explicitly supplied update
    /// clock. Database callers should provide the durable update timestamp;
    /// this does not touch the compressed-content representation.
    pub fn from_message(msg: &ChatMessage, updated_at: u64) -> Result<Self, String> {
        if msg.id.is_empty() || msg.role.is_empty() {
            return Err("Message requires non-empty id and role".to_string());
        }
        validate_safe_non_negative_u64(msg.timestamp, "Message timestamp")?;
        validate_safe_non_negative_u64(updated_at, "Message updatedAt")?;
        let attachments = msg
            .attachments
            .as_ref()
            .map(|items| {
                items
                    .iter()
                    .map(AttachmentSyncDTO::try_from)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;

        let content_hash = msg
            .content_hash
            .as_deref()
            .filter(|hash| is_lowercase_sha256(hash))
            .ok_or_else(|| "Message requires a lowercase SHA-256 contentHash".to_string())?
            .to_string();
        Ok(Self {
            id: msg.id.clone(),
            role: msg.role.clone(),
            name: msg.name.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            updated_at,
            agent_id: msg.agent_id.clone(),
            group_id: msg.group_id.clone(),
            topic_id: msg.topic_id.clone(),
            is_group_message: msg.is_group_message,
            finish_reason: msg.finish_reason.clone(),
            attachments,
            content_hash,
        })
    }

    /// Compatibility conversion for callers that do not yet have a durable
    /// update clock. New sync paths should use [`Self::from_message`].
    pub fn from_message_legacy(msg: &ChatMessage) -> Result<Self, String> {
        Self::from_message(msg, msg.updated_at.unwrap_or(msg.timestamp))
    }
}

impl TryFrom<&ChatMessage> for MessageSyncDTO {
    type Error = String;

    fn try_from(msg: &ChatMessage) -> Result<Self, Self::Error> {
        Self::from_message_legacy(msg)
    }
}

impl From<MessageSyncDTO> for ChatMessage {
    fn from(dto: MessageSyncDTO) -> Self {
        Self {
            id: dto.id,
            role: dto.role,
            name: dto.name,
            content: dto.content,
            timestamp: dto.timestamp,
            updated_at: Some(dto.updated_at),
            is_thinking: None,
            agent_id: dto.agent_id,
            group_id: dto.group_id,
            topic_id: dto.topic_id,
            is_group_message: dto.is_group_message,
            finish_reason: dto.finish_reason,
            attachments: dto.attachments.map(|atts| {
                atts.into_iter()
                    .map(|a| crate::vcp_modules::chat_manager::Attachment {
                        r#type: a.r#type,
                        src: "".to_string(), // 由下游 path_map 填充本地路径
                        name: a.name,
                        size: a.size,
                        hash: Some(a.hash),
                        status: None,
                        attachment_order: a.attachment_order,
                        internal_path: "".to_string(),
                        extracted_text: a.extracted_text,
                        image_frames: a.image_frames,
                        thumbnail_path: None,
                        created_at: a.created_at,
                    })
                    .collect()
            }),
            blocks: None,
            content_hash: Some(dto.content_hash),
            shell: None,
        }
    }
}

fn deserialize_content_hash<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if is_lowercase_sha256(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "contentHash must be a lowercase 64-character SHA-256",
        ))
    }
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        && value.bytes().all(|byte| !byte.is_ascii_uppercase())
}
