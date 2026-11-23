use super::common::{
    deserialize_bounded_vec, deserialize_non_empty_string, MAX_MANIFEST_ITEMS,
    MAX_MESSAGES_PER_TOPIC, MAX_MESSAGE_DIFF_ITEMS, MAX_MESSAGE_DIFF_TOPICS,
};
use super::entity::EntitySelector;
use super::transport::{is_valid_avatar_owner, AvatarOwnerType, OwnerType};
use crate::vcp_modules::infra::utils::is_desktop_compatible_id;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_error::WireSyncError;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug)]
pub enum EntityPullData {
    Agent(AgentSyncDTO),
    Group(GroupSyncDTO),
    GroupTopic(GroupTopicSyncDTO),
    AgentTopic(AgentTopicSyncDTO),
}

#[derive(Debug)]
pub enum EntityPullResult {
    Owner {
        owner_type: OwnerType,
        owner_id: String,
        ok: bool,
        data: Option<EntityPullData>,
        error: Option<WireSyncError>,
    },
    Topic {
        owner_type: OwnerType,
        owner_id: String,
        topic_id: String,
        ok: bool,
        data: Option<EntityPullData>,
        error: Option<WireSyncError>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntityPullResult {
    entity_type: String,
    owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    owner_id: String,
    #[serde(default)]
    topic_id: Option<String>,
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    error: Option<WireSyncError>,
}

impl<'de> Deserialize<'de> for EntityPullResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawEntityPullResult::deserialize(deserializer)?;
        if raw.ok && raw.error.is_some() {
            return Err(D::Error::custom(
                "entity pull result with ok=true must not contain error",
            ));
        }
        if !raw.ok && raw.data.is_some() {
            return Err(D::Error::custom(
                "entity pull result with ok=false must not contain data",
            ));
        }
        match raw.entity_type.as_str() {
            "owner" => {
                if raw.topic_id.is_some() {
                    return Err(D::Error::custom(
                        "owner entity pull result must not contain topicId",
                    ));
                }
                let data = match raw.data {
                    Some(value) => Some(match raw.owner_type {
                        OwnerType::Agent => EntityPullData::Agent(
                            serde_json::from_value(value).map_err(D::Error::custom)?,
                        ),
                        OwnerType::Group => EntityPullData::Group(
                            serde_json::from_value(value).map_err(D::Error::custom)?,
                        ),
                    }),
                    None => None,
                };
                Ok(Self::Owner {
                    owner_type: raw.owner_type,
                    owner_id: raw.owner_id,
                    ok: raw.ok,
                    data,
                    error: raw.error,
                })
            }
            "topic" => {
                let topic_id = raw
                    .topic_id
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        D::Error::custom("topic entity pull result requires non-empty topicId")
                    })?;
                let data = match raw.data {
                    Some(value) => Some(match raw.owner_type {
                        OwnerType::Agent => EntityPullData::AgentTopic(
                            serde_json::from_value(value).map_err(D::Error::custom)?,
                        ),
                        OwnerType::Group => EntityPullData::GroupTopic(
                            serde_json::from_value(value).map_err(D::Error::custom)?,
                        ),
                    }),
                    None => None,
                };
                Ok(Self::Topic {
                    owner_type: raw.owner_type,
                    owner_id: raw.owner_id,
                    topic_id,
                    ok: raw.ok,
                    data,
                    error: raw.error,
                })
            }
            other => Err(D::Error::custom(format!("unsupported entityType {other}"))),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EntityPullResponse {
    #[serde(deserialize_with = "deserialize_bounded_vec")]
    pub results: Vec<EntityPullResult>,
}
impl EntityPullResponse {
    pub fn validate(&self) -> Result<(), String> {
        validate_pull_results(&self.results)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "entityType", rename_all = "lowercase", deny_unknown_fields)]
pub enum EntityPushResult {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId", deserialize_with = "deserialize_non_empty_string")]
        owner_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId", deserialize_with = "deserialize_non_empty_string")]
        owner_id: String,
        #[serde(rename = "topicId", deserialize_with = "deserialize_non_empty_string")]
        topic_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
}

impl EntityPushResult {
    pub fn into_parts(self) -> (EntitySelector, bool, Option<WireSyncError>) {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
                ok,
                error,
            } => (EntitySelector::owner(owner_type, owner_id), ok, error),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
                ok,
                error,
            } => (
                EntitySelector::Topic {
                    owner_type,
                    owner_id,
                    topic_id,
                },
                ok,
                error,
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityPushResponse {
    #[serde(deserialize_with = "deserialize_bounded_vec")]
    pub results: Vec<EntityPushResult>,
}

impl EntityPushResponse {
    pub fn validate(&self) -> Result<(), String> {
        if self.results.len() > MAX_MANIFEST_ITEMS {
            return Err("entity push response exceeds the item budget".to_string());
        }
        let mut identities = BTreeSet::new();
        for result in &self.results {
            let selector = match result {
                EntityPushResult::Owner {
                    owner_type,
                    owner_id,
                    ..
                } => EntitySelector::owner(*owner_type, owner_id),
                EntityPushResult::Topic {
                    owner_type,
                    owner_id,
                    topic_id,
                    ..
                } => EntitySelector::Topic {
                    owner_type: *owner_type,
                    owner_id: owner_id.clone(),
                    topic_id: topic_id.clone(),
                },
            };
            selector.validate()?;
            if !identities.insert(selector.label()) {
                return Err("entity push response contains duplicate identity".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarPushResponse {
    pub owner_type: AvatarOwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    pub ok: bool,
}

impl AvatarPushResponse {
    pub fn validate(&self) -> Result<(), String> {
        if is_valid_avatar_owner(self.owner_type.as_str(), &self.owner_id) {
            Ok(())
        } else {
            Err("avatar response requires a valid avatar identity".to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MessagePushResponseFrame {
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId", deserialize_with = "deserialize_non_empty_string")]
        owner_id: String,
        #[serde(rename = "topicId", deserialize_with = "deserialize_non_empty_string")]
        topic_id: String,
        ok: bool,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
    StreamError {
        error: WireSyncError,
    },
}

impl MessagePushResponseFrame {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Topic {
                owner_id, topic_id, ..
            } if !owner_id.is_empty() && !topic_id.is_empty() => Ok(()),
            Self::StreamError { .. } => Ok(()),
            _ => Err("message response requires complete topic identity".to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePullTopicSelector {
    pub owner_type: OwnerType,
    pub owner_id: String,
    pub topic_id: String,
    pub message_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MessagePullRequest {
    pub topics: Vec<MessagePullTopicSelector>,
}

impl MessagePullRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.topics.len() > MAX_MESSAGE_DIFF_TOPICS {
            return Err("message pull topic count exceeds the budget".to_string());
        }
        let mut identities = BTreeSet::new();
        let mut message_count = 0usize;
        for topic in &self.topics {
            if !matches!(topic.owner_type, OwnerType::Agent | OwnerType::Group)
                || topic.owner_id.is_empty()
                || topic.topic_id.is_empty()
                || topic.message_ids.iter().any(String::is_empty)
            {
                return Err("message pull contains an incomplete identity".to_string());
            }
            if !identities.insert(format!(
                "{}\0{}\0{}",
                topic.owner_type, topic.owner_id, topic.topic_id
            )) {
                return Err("message pull contains duplicate topic identity".to_string());
            }
            if topic.message_ids.len() > MAX_MESSAGES_PER_TOPIC {
                return Err("message pull exceeds the per-topic budget".to_string());
            }
            let mut message_ids = BTreeSet::new();
            if topic
                .message_ids
                .iter()
                .any(|message_id| !message_ids.insert(message_id))
            {
                return Err("message pull contains duplicate message identity".to_string());
            }
            message_count = message_count.saturating_add(topic.message_ids.len());
        }
        if message_count > MAX_MESSAGE_DIFF_ITEMS {
            return Err("message pull exceeds the message budget".to_string());
        }
        Ok(())
    }
}

/// Mobile 内部使用完整身份承载删除目标，避免可选字段组合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteTarget {
    Owner {
        owner_type: OwnerType,
        owner_id: String,
    },
    Topic(TopicKey),
    Avatar {
        owner_type: AvatarOwnerType,
        owner_id: String,
    },
    Message(MessageKey),
}

impl DeleteTarget {
    /// 桌面端身份校验使用的 Owner ID。
    pub fn owner_id(&self) -> &str {
        match self {
            Self::Owner { owner_id, .. } => owner_id,
            Self::Topic(key) => &key.owner_id,
            Self::Avatar { owner_id, .. } => owner_id,
            Self::Message(key) => &key.topic.owner_id,
        }
    }

    /// 桌面插件只接受 `[a-zA-Z0-9_-]` 组成的身份，历史中文 ID 连删除帧都会被拒绝。
    pub fn is_desktop_compatible(&self) -> bool {
        match self {
            Self::Owner { owner_id, .. } | Self::Avatar { owner_id, .. } => {
                is_desktop_compatible_id(owner_id)
            }
            Self::Topic(key) => {
                is_desktop_compatible_id(&key.owner_id) && is_desktop_compatible_id(&key.topic_id)
            }
            Self::Message(key) => {
                is_desktop_compatible_id(&key.topic.owner_id)
                    && is_desktop_compatible_id(&key.topic.topic_id)
                    && is_desktop_compatible_id(&key.msg_id)
            }
        }
    }
}

fn validate_pull_results(results: &[EntityPullResult]) -> Result<(), String> {
    if results.len() > MAX_MANIFEST_ITEMS {
        return Err("entity pull response exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for result in results {
        let selector = match result {
            EntityPullResult::Owner {
                owner_type,
                owner_id,
                ..
            } => EntitySelector::owner(*owner_type, owner_id),
            EntityPullResult::Topic {
                owner_type,
                owner_id,
                topic_id,
                ..
            } => EntitySelector::Topic {
                owner_type: *owner_type,
                owner_id: owner_id.clone(),
                topic_id: topic_id.clone(),
            },
        };
        selector.validate()?;
        if !identities.insert(selector.label()) {
            return Err("entity pull response contains duplicate identity".to_string());
        }
    }
    Ok(())
}
