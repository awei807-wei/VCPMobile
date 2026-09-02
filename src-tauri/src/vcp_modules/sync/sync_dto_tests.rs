use super::*;
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::topic_types::Topic;
use serde_json::json;

#[test]
fn test_agent_topic_sync_dto_deserialization_defaults_locked_and_unread() {
    let dto: AgentTopicSyncDTO = serde_json::from_value(json!({
        "id": "topic-1",
        "name": "Topic",
        "createdAt": 123,
        "ownerId": "agent-1"
    }))
    .unwrap();

    assert!(dto.locked);
    assert!(!dto.unread);
}

#[test]
fn test_agent_and_group_topic_dto_from_topic_preserve_contract_fields() {
    let topic = Topic {
        id: "topic-1".to_string(),
        name: "Topic".to_string(),
        created_at: 123,
        locked: false,
        unread: true,
        unread_count: 2,
        msg_count: 3,
        owner_id: "owner-1".to_string(),
        owner_type: "agent".to_string(),
    };

    let agent_dto = AgentTopicSyncDTO::from(&topic);
    assert_eq!(agent_dto.id, "topic-1");
    assert_eq!(agent_dto.name, "Topic");
    assert_eq!(agent_dto.created_at, 123);
    assert!(!agent_dto.locked);
    assert!(agent_dto.unread);
    assert_eq!(agent_dto.owner_id, "owner-1");

    let group_dto = GroupTopicSyncDTO::from(&topic);
    assert_eq!(group_dto.id, "topic-1");
    assert_eq!(group_dto.name, "Topic");
    assert_eq!(group_dto.created_at, 123);
    assert_eq!(group_dto.owner_id, "owner-1");
}

#[test]
fn test_attachment_sync_dto_from_attachment_preserves_sync_fields_only() {
    let attachment = Attachment {
        r#type: "image".to_string(),
        src: "/local/path.png".to_string(),
        name: "path.png".to_string(),
        size: 42,
        hash: Some("A".repeat(64)),
        status: Some("ready".to_string()),
        internal_path: "internal/path.png".to_string(),
        extracted_text: Some("text".to_string()),
        image_frames: Some(vec!["frame-1".to_string()]),
        thumbnail_path: Some("thumb.png".to_string()),
        created_at: Some(100),
    };

    let dto = AttachmentSyncDTO::try_from(&attachment).expect("valid attachment DTO");
    assert_eq!(dto.r#type, "image");
    assert_eq!(dto.name, "path.png");
    assert_eq!(dto.size, 42);
    assert_eq!(dto.hash, "a".repeat(64));
    assert_eq!(dto.status.as_deref(), Some("ready"));
    assert_eq!(dto.extracted_text.as_deref(), Some("text"));
    assert_eq!(dto.image_frames.as_ref().unwrap()[0], "frame-1");
    assert_eq!(dto.created_at, Some(100));

    let json = serde_json::to_value(&dto).unwrap();
    let obj = json.as_object().unwrap();
    assert!(!obj.contains_key("src"));
    assert!(!obj.contains_key("internalPath"));
    assert!(!obj.contains_key("thumbnailPath"));
}

#[test]
fn attachment_sync_dto_rejects_missing_or_invalid_hash() {
    let mut attachment = Attachment {
        r#type: "file".to_string(),
        src: String::new(),
        name: "missing.bin".to_string(),
        size: 1,
        hash: None,
        status: None,
        internal_path: String::new(),
        extracted_text: None,
        image_frames: None,
        thumbnail_path: None,
        created_at: None,
    };
    assert!(AttachmentSyncDTO::try_from(&attachment).is_err());
    attachment.hash = Some("not-a-sha256".to_string());
    assert!(AttachmentSyncDTO::try_from(&attachment).is_err());
}

#[test]
fn test_message_pull_sync_dto_into_chat_message_maps_attachments_and_defaults_local_paths() {
    let dto = MessagePullSyncDTO {
        id: "msg-1".to_string(),
        role: "user".to_string(),
        name: Some("User".to_string()),
        content: "hello".to_string(),
        timestamp: 123,
        is_thinking: Some(false),
        agent_id: Some("agent-1".to_string()),
        group_id: Some("group-1".to_string()),
        topic_id: Some("topic-1".to_string()),
        is_group_message: Some(true),
        finish_reason: Some("stop".to_string()),
        attachments: Some(vec![AttachmentSyncDTO {
            r#type: "file".to_string(),
            name: "a.txt".to_string(),
            size: 10,
            hash: "hash-a".to_string(),
            status: Some("ready".to_string()),
            extracted_text: Some("extracted".to_string()),
            image_frames: None,
            created_at: Some(200),
        }]),
        content_hash: Some("content-hash".to_string()),
        avatar_color: Some("#fff".to_string()),
    };

    let msg = ChatMessage::from(dto);
    assert_eq!(msg.id, "msg-1");
    assert_eq!(msg.role, "user");
    assert_eq!(msg.name.as_deref(), Some("User"));
    assert_eq!(msg.content, "hello");
    assert_eq!(msg.timestamp, 123);
    assert_eq!(msg.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(msg.group_id.as_deref(), Some("group-1"));
    assert_eq!(msg.topic_id.as_deref(), Some("topic-1"));
    assert_eq!(msg.is_group_message, Some(true));
    assert_eq!(msg.content_hash.as_deref(), Some("content-hash"));
    assert!(msg.blocks.is_none());
    assert!(msg.shell.is_none());

    let attachment = &msg.attachments.as_ref().unwrap()[0];
    assert_eq!(attachment.r#type, "file");
    assert_eq!(attachment.name, "a.txt");
    assert_eq!(attachment.hash.as_deref(), Some("hash-a"));
    assert_eq!(attachment.src, "");
    assert_eq!(attachment.internal_path, "");
    assert!(attachment.thumbnail_path.is_none());
}
