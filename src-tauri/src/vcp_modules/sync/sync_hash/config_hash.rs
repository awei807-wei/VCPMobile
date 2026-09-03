use super::canonical::{compute_canonical_hash, object, string};
use super::{HashAggregator, DEFAULT_INVITE_PROMPT};
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO};
use serde_json::Value;

impl HashAggregator {
    pub fn compute_agent_config_hash(dto: &AgentSyncDTO) -> String {
        let temperature = (dto.temperature * 100.0).round() / 100.0;
        compute_canonical_hash(&object([
            ("name", string(&dto.name)),
            ("systemPrompt", string(&dto.system_prompt)),
            ("model", string(&dto.model)),
            (
                "temperature",
                serde_json::to_value(temperature).unwrap_or(Value::Null),
            ),
            ("contextTokenLimit", dto.context_token_limit.into()),
            ("maxOutputTokens", dto.max_output_tokens.into()),
            ("streamOutput", Value::Bool(dto.stream_output)),
        ]))
    }

    pub fn compute_group_config_hash(dto: &GroupSyncDTO) -> String {
        compute_canonical_hash(&object([
            ("name", string(&dto.name)),
            (
                "members",
                Value::Array(dto.members.iter().cloned().map(string).collect()),
            ),
            ("mode", string(&dto.mode)),
            (
                "memberTags",
                dto.member_tags.clone().unwrap_or_else(|| object([])),
            ),
            (
                "groupPrompt",
                string(dto.group_prompt.as_deref().unwrap_or("")),
            ),
            (
                "invitePrompt",
                string(
                    dto.invite_prompt
                        .as_deref()
                        .unwrap_or(DEFAULT_INVITE_PROMPT),
                ),
            ),
            ("useUnifiedModel", Value::Bool(dto.use_unified_model)),
            (
                "unifiedModel",
                string(dto.unified_model.as_deref().unwrap_or("")),
            ),
            (
                "tagMatchMode",
                string(dto.tag_match_mode.as_deref().unwrap_or("strict")),
            ),
            ("createdAt", dto.created_at.into()),
        ]))
    }

    pub fn compute_avatar_hash(bytes: &[u8]) -> String {
        crate::vcp_modules::infra::utils::calculate_sha256(bytes)
    }

    /// Existing local content hash used by render/parser code. Wire message
    /// fingerprints use SHA-256 and should call the methods above.
    pub fn compute_content_hash(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}
