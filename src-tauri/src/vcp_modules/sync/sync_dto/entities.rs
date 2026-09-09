use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_types::{deserialize_timestamp, serialize_timestamp};
use crate::vcp_modules::topic_types::Topic;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Agent configuration exposed by the sync contract.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSyncDTO {
    pub name: String,
    pub system_prompt: String,
    pub model: String,
    pub temperature: f64,
    pub context_token_limit: i32,
    pub max_output_tokens: i32,
    pub stream_output: bool,
}

impl From<&AgentConfig> for AgentSyncDTO {
    fn from(config: &AgentConfig) -> Self {
        Self {
            name: config.name.clone(),
            system_prompt: config.system_prompt.clone(),
            model: config.model.clone(),
            temperature: config.temperature,
            context_token_limit: config.context_token_limit,
            max_output_tokens: config.max_output_tokens,
            stream_output: config.stream_output,
        }
    }
}

/// Group configuration exposed by the sync contract.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupSyncDTO {
    pub name: String,
    pub members: Vec<String>,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_tags: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_prompt: Option<String>,
    pub use_unified_model: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unified_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_match_mode: Option<String>,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub created_at: i64,
}

impl From<&GroupConfig> for GroupSyncDTO {
    fn from(config: &GroupConfig) -> Self {
        Self {
            name: config.name.clone(),
            members: config.members.clone(),
            mode: config.mode.clone(),
            member_tags: normalize_member_tags(config.member_tags.as_ref()),
            group_prompt: config.group_prompt.clone(),
            invite_prompt: config.invite_prompt.clone(),
            use_unified_model: config.use_unified_model,
            unified_model: config.unified_model.clone(),
            tag_match_mode: config.tag_match_mode.clone(),
            created_at: config.created_at,
        }
    }
}

/// 规范化 Wire 中的成员标签；空字符串、空白字符串和非字符串值都表示无标签。
/// 这样配置哈希只覆盖真正可执行的标签，避免数据库与 DTO 产生不同摘要。
pub fn normalize_member_tags(value: Option<&Value>) -> Option<Value> {
    let object = value?.as_object()?;
    let mut tags = Map::new();
    for (agent_id, tag) in object {
        let Some(tag) = tag.as_str() else {
            continue;
        };
        if !tag.trim().is_empty() {
            tags.insert(agent_id.clone(), Value::String(tag.to_string()));
        }
    }
    Some(Value::Object(tags))
}

/// Agent topic sync DTO, including its lock and unread state.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTopicSyncDTO {
    pub id: String,
    pub name: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub created_at: i64,
    #[serde(default = "default_locked")]
    pub locked: bool,
    #[serde(default = "default_unread")]
    pub unread: bool,
    pub owner_id: String,
}

fn default_locked() -> bool {
    true
}

fn default_unread() -> bool {
    false
}

impl From<&Topic> for AgentTopicSyncDTO {
    fn from(topic: &Topic) -> Self {
        Self {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            locked: topic.locked,
            unread: topic.unread,
            owner_id: topic.owner_id.clone(),
        }
    }
}

/// Group topic sync DTO.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupTopicSyncDTO {
    pub id: String,
    pub name: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub created_at: i64,
    pub owner_id: String,
}

impl From<&Topic> for GroupTopicSyncDTO {
    fn from(topic: &Topic) -> Self {
        Self {
            id: topic.id.clone(),
            name: topic.name.clone(),
            created_at: topic.created_at,
            owner_id: topic.owner_id.clone(),
        }
    }
}
