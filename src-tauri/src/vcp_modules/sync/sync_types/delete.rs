use super::common::serialize_timestamp;
use super::entity_wire::DeleteTarget;
use super::transport::{AvatarOwnerType, OwnerType};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "targetType", rename_all = "lowercase")]
enum DeleteNotificationTarget {
    Owner {
        #[serde(rename = "ownerType")]
        owner_type: OwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Topic {
        #[serde(rename = "ownerType")]
        owner_type: String,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
    },
    Avatar {
        #[serde(rename = "ownerType")]
        owner_type: AvatarOwnerType,
        #[serde(rename = "ownerId")]
        owner_id: String,
    },
    Message {
        #[serde(rename = "ownerType")]
        owner_type: String,
        #[serde(rename = "ownerId")]
        owner_id: String,
        #[serde(rename = "topicId")]
        topic_id: String,
        #[serde(rename = "msgId")]
        msg_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNotificationFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    #[serde(flatten)]
    target: DeleteNotificationTarget,
    #[serde(serialize_with = "serialize_timestamp")]
    deleted_at: i64,
}

impl DeleteNotificationFrame {
    pub fn new(target: DeleteTarget, deleted_at: i64) -> Self {
        let target = match target {
            DeleteTarget::Owner {
                owner_type,
                owner_id,
            } => DeleteNotificationTarget::Owner {
                owner_type,
                owner_id,
            },
            DeleteTarget::Topic(key) => DeleteNotificationTarget::Topic {
                owner_type: key.owner_type,
                owner_id: key.owner_id,
                topic_id: key.topic_id,
            },
            DeleteTarget::Avatar {
                owner_type,
                owner_id,
            } => DeleteNotificationTarget::Avatar {
                owner_type,
                owner_id,
            },
            DeleteTarget::Message(key) => DeleteNotificationTarget::Message {
                owner_type: key.topic.owner_type,
                owner_id: key.topic.owner_id,
                topic_id: key.topic.topic_id,
                msg_id: key.msg_id,
            },
        };
        Self {
            frame_type: "SYNC_ENTITY_DELETE",
            target,
            deleted_at,
        }
    }
}
