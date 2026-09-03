use super::common::{
    deserialize_bounded_vec, deserialize_non_empty_string, MAX_MANIFEST_ITEMS,
    MAX_MESSAGES_PER_TOPIC, MAX_MESSAGE_DIFF_ITEMS, MAX_MESSAGE_DIFF_TOPICS,
};
use super::entity::EntitySelector;
use super::transport::{is_valid_avatar_owner, AvatarOwnerType, OwnerType};
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_error::WireSyncError;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EntityPullData {
    Agent(AgentSyncDTO),
    Group(GroupSyncDTO),
    GroupTopic(GroupTopicSyncDTO),
    AgentTopic(AgentTopicSyncDTO),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "entityType", rename_all = "lowercase", deny_unknown_fields)]
pub enum EntityPullResult {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId", deserialize_with = "deserialize_non_empty_string")]
        owner_id: String,
        ok: bool,
        #[serde(default)]
        data: Option<EntityPullData>,
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
        data: Option<EntityPullData>,
        #[serde(default)]
        error: Option<WireSyncError>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
