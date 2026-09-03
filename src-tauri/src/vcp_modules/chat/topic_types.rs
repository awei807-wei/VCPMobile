use serde::{Deserialize, Serialize};

/// Stable identity for an owner across agent and group namespaces.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerKey {
    pub owner_type: String,
    pub owner_id: String,
}

impl OwnerKey {
    /// Construct an owner identity without normalizing protocol values.
    pub fn new(owner_type: impl Into<String>, owner_id: impl Into<String>) -> Self {
        Self {
            owner_type: owner_type.into(),
            owner_id: owner_id.into(),
        }
    }

    /// Return whether this key is valid for chat owners on Wire 1.4.
    pub fn is_valid(&self) -> bool {
        matches!(self.owner_type.as_str(), "agent" | "group") && !self.owner_id.is_empty()
    }
}

/// Stable identity for a topic, including its owner namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicKey {
    pub owner_type: String,
    pub owner_id: String,
    pub topic_id: String,
}

impl TopicKey {
    /// Construct a composite topic identity without normalizing protocol values.
    pub fn new(
        owner_type: impl Into<String>,
        owner_id: impl Into<String>,
        topic_id: impl Into<String>,
    ) -> Self {
        Self {
            owner_type: owner_type.into(),
            owner_id: owner_id.into(),
            topic_id: topic_id.into(),
        }
    }

    /// Return whether every identity component is valid for Wire 1.4 chat data.
    pub fn is_valid(&self) -> bool {
        matches!(self.owner_type.as_str(), "agent" | "group")
            && !self.owner_id.is_empty()
            && !self.topic_id.is_empty()
    }

    /// Borrow the owner portion of this topic identity.
    pub fn owner_key(&self) -> OwnerKey {
        OwnerKey::new(self.owner_type.clone(), self.owner_id.clone())
    }
}

/// Stable identity for a message inside a composite topic namespace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageKey {
    pub topic: TopicKey,
    pub msg_id: String,
}

impl MessageKey {
    /// Construct a message identity from its topic and message id.
    pub fn new(topic: TopicKey, msg_id: impl Into<String>) -> Self {
        Self {
            topic,
            msg_id: msg_id.into(),
        }
    }

    /// Return whether the topic and message components are valid.
    pub fn is_valid(&self) -> bool {
        self.topic.is_valid() && !self.msg_id.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicActivityDto {
    pub msg_count: i32,
    pub updated_at: i64,
}

/// Resolve a topic's visible activity time from message, topic and creation clocks.
pub fn resolve_topic_activity_updated_at(
    topic_updated_at: i64,
    last_message_updated_at: i64,
    created_at: i64,
) -> i64 {
    let topic_activity = if topic_updated_at > 0 {
        topic_updated_at
    } else {
        created_at
    };
    if last_message_updated_at > 0 {
        topic_activity.max(last_message_updated_at)
    } else {
        topic_activity
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Topic {
    pub id: String,
    pub name: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: i64,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub unread: bool,
    #[serde(rename = "unreadCount", default)]
    pub unread_count: i32,
    #[serde(rename = "msgCount", default)]
    pub msg_count: i32,
    #[serde(rename = "ownerId")]
    pub owner_id: String,
    #[serde(rename = "ownerType")]
    pub owner_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn composite_keys_require_complete_supported_identity() {
        let owner = OwnerKey::new("agent", "agent-1");
        let topic = TopicKey::new("agent", "agent-1", "shared-topic");
        let message = MessageKey::new(topic.clone(), "message-1");

        assert!(owner.is_valid());
        assert!(topic.is_valid());
        assert!(message.is_valid());
        assert_eq!(topic.owner_key(), owner);
        assert!(!TopicKey::new("user", "user_avatar", "topic").is_valid());
        assert!(!TopicKey::new("group", "", "topic").is_valid());
        assert!(!MessageKey::new(topic, "").is_valid());
    }

    #[test]
    fn topic_requires_owner_identity_and_uses_camel_case() {
        let topic: Topic = serde_json::from_value(json!({
            "id": "topic-1",
            "name": "Topic",
            "createdAt": 123,
            "ownerId": "agent-1",
            "ownerType": "agent"
        }))
        .unwrap();
        assert_eq!(topic.owner_id, "agent-1");
        assert_eq!(topic.owner_type, "agent");
        assert!(serde_json::from_value::<Topic>(json!({
            "id": "topic-1",
            "name": "Topic"
        }))
        .is_err());
    }

    #[test]
    fn topic_activity_uses_live_message_then_topic_then_creation_fallback() {
        assert_eq!(resolve_topic_activity_updated_at(200, 300, 100), 300);
        assert_eq!(resolve_topic_activity_updated_at(400, 300, 100), 400);
        assert_eq!(resolve_topic_activity_updated_at(200, 0, 100), 200);
        assert_eq!(resolve_topic_activity_updated_at(0, 0, 100), 100);
    }
}
