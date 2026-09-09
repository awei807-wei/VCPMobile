use super::common::{deserialize_non_empty_string, MAX_MANIFEST_ITEMS};
use super::transport::OwnerType;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::topic_types::TopicKey;
use serde::Serialize;
use std::collections::BTreeSet;

/// 公共 HTTP Entity 选择器。Owner 与 Topic 始终携带完整身份。
#[derive(Debug, Serialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(tag = "entityType", rename_all = "lowercase")]
pub enum EntitySelector {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId", deserialize_with = "deserialize_non_empty_string")]
        owner_id: String,
        #[serde(rename = "topicId", deserialize_with = "deserialize_non_empty_string")]
        topic_id: String,
    },
}

impl EntitySelector {
    pub fn owner(owner_type: OwnerType, owner_id: impl Into<String>) -> Self {
        Self::Owner {
            owner_type,
            owner_id: owner_id.into(),
        }
    }

    pub fn topic(key: &TopicKey) -> Result<Self, String> {
        let owner_type = OwnerType::try_from(key.owner_type.as_str())
            .map_err(|_| "Entity topic selector requires agent/group ownerType".to_string())?;
        if key.owner_id.is_empty() || key.topic_id.is_empty() {
            return Err("Entity topic selector requires complete identity".to_string());
        }
        Ok(Self::Topic {
            owner_type,
            owner_id: key.owner_id.clone(),
            topic_id: key.topic_id.clone(),
        })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
            } => format!("owner/{owner_type}/{owner_id}"),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
            } => format!("topic/{owner_type}/{owner_id}/{topic_id}"),
        }
    }

    pub fn topic_id(&self) -> Option<&str> {
        match self {
            Self::Owner { .. } => None,
            Self::Topic { topic_id, .. } => Some(topic_id),
        }
    }

    /// Validate that the selector carries a complete Wire 1.4 identity.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
            } if !owner_id.is_empty()
                && matches!(owner_type, OwnerType::Agent | OwnerType::Group) =>
            {
                Ok(())
            }
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
            } if !owner_id.is_empty()
                && !topic_id.is_empty()
                && matches!(owner_type, OwnerType::Agent | OwnerType::Group) =>
            {
                Ok(())
            }
            _ => Err("entity selector requires complete identity".to_string()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct EntityPullRequest {
    pub items: Vec<EntitySelector>,
}

impl EntityPullRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_bounded_selectors(&self.items)
    }
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
pub enum EntityPushData {
    Agent(AgentSyncDTO),
    Group(GroupSyncDTO),
    AgentTopic(AgentTopicSyncDTO),
    GroupTopic(GroupTopicSyncDTO),
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "entityType", rename_all = "lowercase")]
pub enum EntityPushItem {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        data: EntityPushData,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        data: EntityPushData,
    },
}

impl EntityPushItem {
    pub fn selector(&self) -> EntitySelector {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
                ..
            } => EntitySelector::owner(*owner_type, owner_id),
            Self::Topic {
                owner_type,
                owner_id,
                topic_id,
                ..
            } => EntitySelector::Topic {
                owner_type: *owner_type,
                owner_id: owner_id.clone(),
                topic_id: topic_id.clone(),
            },
        }
    }

    pub fn is_consistent(&self) -> bool {
        match self {
            Self::Owner {
                owner_type: OwnerType::Agent,
                owner_id,
                data: EntityPushData::Agent(_),
            }
            | Self::Owner {
                owner_type: OwnerType::Group,
                owner_id,
                data: EntityPushData::Group(_),
            } => !owner_id.is_empty(),
            Self::Topic {
                owner_type: OwnerType::Agent,
                owner_id,
                topic_id,
                data: EntityPushData::AgentTopic(data),
            } => topic_payload_is_consistent(owner_id, topic_id, &data.id, &data.owner_id),
            Self::Topic {
                owner_type: OwnerType::Group,
                owner_id,
                topic_id,
                data: EntityPushData::GroupTopic(data),
            } => topic_payload_is_consistent(owner_id, topic_id, &data.id, &data.owner_id),
            _ => false,
        }
    }
}

fn topic_payload_is_consistent(
    owner_id: &str,
    topic_id: &str,
    payload_topic_id: &str,
    payload_owner_id: &str,
) -> bool {
    !owner_id.is_empty()
        && !topic_id.is_empty()
        && payload_topic_id == topic_id
        && payload_owner_id == owner_id
}

#[derive(Debug, Serialize)]
pub struct EntityPushRequest {
    pub items: Vec<EntityPushItem>,
}

impl EntityPushRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.items.len() > MAX_MANIFEST_ITEMS {
            return Err("entity push exceeds the item budget".to_string());
        }
        let mut identities = BTreeSet::new();
        for item in &self.items {
            if !item.is_consistent() {
                return Err("entity push contains an inconsistent identity or payload".to_string());
            }
            if !identities.insert(item.selector().label()) {
                return Err("entity push contains duplicate identity".to_string());
            }
        }
        Ok(())
    }
}

fn validate_bounded_selectors(items: &[EntitySelector]) -> Result<(), String> {
    if items.len() > MAX_MANIFEST_ITEMS {
        return Err("entity selector count exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for item in items {
        item.validate()?;
        if !identities.insert(item.label()) {
            return Err("entity selectors contain duplicate identity".to_string());
        }
    }
    Ok(())
}
