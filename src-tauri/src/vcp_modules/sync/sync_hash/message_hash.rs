use super::canonical::{compute_canonical_hash, object, string};
use super::HashAggregator;
use crate::vcp_modules::sync_dto::{
    AgentTopicSyncDTO, AttachmentSyncDTO, GroupTopicSyncDTO, MessageSyncDTO,
};
use serde_json::Value;

impl HashAggregator {
    /// Legacy two-argument helper retained for non-sync render code. New
    /// persistence paths must use `compute_message_fingerprint_with_identity`
    /// or `compute_message_fingerprint_for_dto` so message identity is part of
    /// the fingerprint.
    pub fn compute_message_fingerprint(content: &str, attachment_hashes: &[String]) -> String {
        Self::compute_message_fingerprint_with_identity(
            "",
            "",
            None,
            content,
            0,
            None,
            attachment_hashes,
        )
    }

    /// Compute the exact desktop `computeMessageFingerprint` payload.
    pub fn compute_message_fingerprint_with_identity(
        message_id: &str,
        role: &str,
        name: Option<&str>,
        content: &str,
        timestamp: u64,
        agent_id: Option<&str>,
        attachment_hashes: &[String],
    ) -> String {
        let mut fields = vec![
            ("id", string(message_id)),
            ("role", string(role)),
            ("content", string(content)),
            ("timestamp", Value::Number(timestamp.into())),
        ];
        if let Some(name) = name {
            fields.push(("name", string(name)));
        }
        if let Some(agent_id) = agent_id {
            fields.push(("agentId", string(agent_id)));
        }

        let mut sorted_hashes = attachment_hashes
            .iter()
            .filter(|hash| !hash.is_empty())
            .cloned()
            .collect::<Vec<_>>();
        sorted_hashes.sort_unstable();
        if !sorted_hashes.is_empty() {
            fields.push((
                "attachmentHashes",
                Value::Array(sorted_hashes.into_iter().map(string).collect()),
            ));
        }

        compute_canonical_hash(&object(fields))
    }

    /// Compute the fingerprint for the canonical message DTO. `groupId`,
    /// `topicId`, and `isGroupMessage` intentionally remain outside the
    /// payload to match the desktop's topic-scoped fingerprint contract.
    pub fn compute_message_fingerprint_for_dto(dto: &MessageSyncDTO) -> String {
        let attachment_hashes = dto
            .attachments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|attachment: &AttachmentSyncDTO| attachment.hash.clone())
            .collect::<Vec<_>>();
        Self::compute_message_fingerprint_with_identity(
            &dto.id,
            &dto.role,
            dto.name.as_deref(),
            &dto.content,
            dto.timestamp,
            dto.agent_id.as_deref(),
            &attachment_hashes,
        )
    }

    pub fn compute_message_leaf_hash(message_id: &str, message_hash: &str) -> String {
        compute_canonical_hash(&object([
            ("id", string(message_id)),
            ("hash", string(message_hash)),
        ]))
    }

    pub fn compute_topic_leaf_hash(
        topic_id: &str,
        config_hash: &str,
        content_hash: &str,
    ) -> String {
        compute_canonical_hash(&object([
            ("topicId", string(topic_id)),
            ("configHash", string(config_hash)),
            ("contentHash", string(content_hash)),
        ]))
    }

    pub fn compute_agent_topic_metadata_hash(dto: &AgentTopicSyncDTO) -> String {
        compute_canonical_hash(&object([
            ("id", string(&dto.id)),
            ("name", string(&dto.name)),
            ("createdAt", dto.created_at.into()),
            ("locked", Value::Bool(dto.locked)),
            ("unread", Value::Bool(dto.unread)),
        ]))
    }

    pub fn compute_group_topic_metadata_hash(dto: &GroupTopicSyncDTO) -> String {
        compute_canonical_hash(&object([
            ("id", string(&dto.id)),
            ("name", string(&dto.name)),
            ("createdAt", dto.created_at.into()),
        ]))
    }
}
