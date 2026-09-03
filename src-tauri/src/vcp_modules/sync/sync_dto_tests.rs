use super::*;
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::topic_types::Topic;
use serde_json::json;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MAX_SAFE_JSON_INTEGER: u64 = crate::vcp_modules::sync::sync_types::MAX_SAFE_TIMESTAMP as u64;

#[test]
fn topic_dto_defaults_are_preserved_while_unknown_fields_are_rejected() {
    let dto: AgentTopicSyncDTO = serde_json::from_value(json!({
        "id": "topic-1",
        "name": "Topic",
        "createdAt": 123,
        "ownerId": "agent-1"
    }))
    .expect("legacy-compatible topic defaults");

    assert!(dto.locked);
    assert!(!dto.unread);
    assert!(serde_json::from_value::<AgentTopicSyncDTO>(json!({
        "id": "topic-1",
        "name": "Topic",
        "createdAt": 123,
        "ownerId": "agent-1",
        "desktopOnly": true
    }))
    .is_err());
}

#[test]
fn topic_dto_from_topic_preserves_owner_identity_and_contract_fields() {
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
    assert_eq!(group_dto.owner_id, "owner-1");
    assert_eq!(group_dto.created_at, 123);
}

#[test]
fn attachment_sync_dto_contains_only_canonical_metadata() {
    let attachment = Attachment {
        r#type: "image".to_string(),
        src: "/local/path.png".to_string(),
        name: "path.png".to_string(),
        size: 42,
        hash: Some(HASH_A.to_ascii_uppercase()),
        status: Some("ready".to_string()),
        attachment_order: Some(3),
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
    assert_eq!(dto.hash, HASH_A);
    assert_eq!(dto.attachment_order, Some(3));
    assert_eq!(dto.extracted_text.as_deref(), Some("text"));
    assert_eq!(dto.image_frames.as_ref().unwrap()[0], "frame-1");
    assert_eq!(dto.created_at, Some(100));

    let value = serde_json::to_value(&dto).expect("serialize attachment DTO");
    let object = value.as_object().expect("attachment object");
    assert!(!object.contains_key("src"));
    assert!(!object.contains_key("internalPath"));
    assert!(!object.contains_key("thumbnailPath"));
    assert!(!object.contains_key("status"));
    assert!(!object.contains_key("_fileManagerData"));
}

#[test]
fn attachment_sync_dto_rejects_invalid_hash_and_unknown_local_fields() {
    let invalid_hash = serde_json::from_value::<AttachmentSyncDTO>(json!({
        "type": "file",
        "name": "broken.bin",
        "size": 1,
        "hash": "not-a-sha256"
    }));
    assert!(invalid_hash.is_err());

    let local_field = serde_json::from_value::<AttachmentSyncDTO>(json!({
        "type": "file",
        "name": "file.bin",
        "size": 1,
        "hash": HASH_A,
        "src": "/desktop/path"
    }));
    assert!(local_field.is_err());

    let local_status = serde_json::from_value::<AttachmentSyncDTO>(json!({
        "type": "file",
        "name": "file.bin",
        "size": 1,
        "hash": HASH_A,
        "status": "ready"
    }));
    assert!(local_status.is_err());

    let mut attachment = Attachment {
        r#type: "file".to_string(),
        src: String::new(),
        name: "missing.bin".to_string(),
        size: 1,
        hash: None,
        status: None,
        attachment_order: None,
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
fn message_sync_dto_requires_updated_at_and_rejects_local_fields() {
    let base = json!({
        "id": "message-1",
        "role": "user",
        "content": "hello",
        "timestamp": 123,
        "updatedAt": 124
    });
    let dto: MessageSyncDTO = serde_json::from_value(base.clone()).expect("valid message DTO");
    assert_eq!(dto.updated_at, 124);

    let mut missing_updated_at = base.as_object().unwrap().clone();
    missing_updated_at.remove("updatedAt");
    assert!(serde_json::from_value::<MessageSyncDTO>(json!(missing_updated_at)).is_err());

    for local_field in ["avatarColor", "status", "deletedAt", "blocks"] {
        let mut value = base.as_object().unwrap().clone();
        value.insert(local_field.to_string(), json!("local-only"));
        assert!(
            serde_json::from_value::<MessageSyncDTO>(json!(value)).is_err(),
            "unexpectedly accepted local field {local_field}"
        );
    }
}

#[test]
fn wire_numeric_fields_match_javascript_safe_integer_contract() {
    let base = json!({
        "id": "message-1",
        "role": "user",
        "content": "hello",
        "timestamp": MAX_SAFE_JSON_INTEGER,
        "updatedAt": MAX_SAFE_JSON_INTEGER,
        "attachments": [{
            "type": "file",
            "name": "payload.bin",
            "size": MAX_SAFE_JSON_INTEGER,
            "hash": HASH_A,
            "createdAt": MAX_SAFE_JSON_INTEGER
        }]
    });
    let dto: MessageSyncDTO = serde_json::from_value(base.clone())
        .expect("2^53 - 1 is representable by JavaScript Number");
    assert_eq!(dto.timestamp, MAX_SAFE_JSON_INTEGER);
    assert_eq!(dto.updated_at, MAX_SAFE_JSON_INTEGER);
    let attachment = &dto.attachments.as_ref().expect("attachment")[0];
    assert_eq!(attachment.size, MAX_SAFE_JSON_INTEGER);
    assert_eq!(attachment.created_at, Some(MAX_SAFE_JSON_INTEGER));

    for field in ["timestamp", "updatedAt"] {
        let mut value = base.as_object().expect("message object").clone();
        value.insert(field.to_string(), json!(MAX_SAFE_JSON_INTEGER + 1));
        assert!(
            serde_json::from_value::<MessageSyncDTO>(json!(value)).is_err(),
            "2^53 must be rejected for {field}"
        );
    }
    for field in ["size", "createdAt"] {
        let mut value = base.as_object().expect("message object").clone();
        value["attachments"][0][field] = json!(MAX_SAFE_JSON_INTEGER + 1);
        assert!(
            serde_json::from_value::<MessageSyncDTO>(json!(value)).is_err(),
            "2^53 must be rejected for attachment {field}"
        );
    }
}

#[test]
fn entity_created_at_uses_the_same_safe_integer_contract() {
    let maximum: AgentTopicSyncDTO = serde_json::from_value(json!({
        "id": "topic-1",
        "name": "Topic",
        "createdAt": MAX_SAFE_JSON_INTEGER,
        "ownerId": "agent-1"
    }))
    .expect("safe createdAt should decode");
    assert_eq!(maximum.created_at, MAX_SAFE_JSON_INTEGER as i64);
    assert!(serde_json::from_value::<AgentTopicSyncDTO>(json!({
        "id": "topic-1",
        "name": "Topic",
        "createdAt": MAX_SAFE_JSON_INTEGER + 1,
        "ownerId": "agent-1"
    }))
    .is_err());
    let unsafe_outbound = AgentTopicSyncDTO {
        created_at: MAX_SAFE_JSON_INTEGER as i64 + 1,
        ..maximum
    };
    assert!(serde_json::to_value(unsafe_outbound).is_err());
}

#[test]
fn wire_numeric_serialization_rejects_values_that_deserialization_rejects() {
    let mut dto: MessageSyncDTO = serde_json::from_value(json!({
        "id": "message-1",
        "role": "user",
        "content": "hello",
        "timestamp": MAX_SAFE_JSON_INTEGER,
        "updatedAt": MAX_SAFE_JSON_INTEGER
    }))
    .expect("safe message DTO");
    dto.timestamp = MAX_SAFE_JSON_INTEGER + 1;
    assert!(serde_json::to_value(&dto).is_err());
    dto.timestamp = MAX_SAFE_JSON_INTEGER;
    dto.updated_at = MAX_SAFE_JSON_INTEGER + 1;
    assert!(serde_json::to_value(&dto).is_err());

    let message = ChatMessage {
        id: "message-1".to_string(),
        role: "user".to_string(),
        content: "hello".to_string(),
        timestamp: MAX_SAFE_JSON_INTEGER + 1,
        ..ChatMessage::default()
    };
    assert!(MessageSyncDTO::from_message(&message, MAX_SAFE_JSON_INTEGER).is_err());

    let attachment = AttachmentSyncDTO {
        r#type: "file".to_string(),
        name: "payload.bin".to_string(),
        size: MAX_SAFE_JSON_INTEGER + 1,
        hash: HASH_A.to_string(),
        attachment_order: None,
        extracted_text: None,
        image_frames: None,
        created_at: Some(MAX_SAFE_JSON_INTEGER),
        status: None,
    };
    assert!(serde_json::to_value(&attachment).is_err());
}

#[test]
fn message_sync_dto_round_trip_maps_local_attachment_paths_to_empty_values() {
    let dto: MessageSyncDTO = serde_json::from_value(json!({
        "id": "msg-1",
        "role": "assistant",
        "name": "Nova",
        "content": "hello",
        "timestamp": 123,
        "updatedAt": 124,
        "isThinking": false,
        "agentId": "agent-1",
        "groupId": "group-1",
        "topicId": "topic-1",
        "isGroupMessage": true,
        "finishReason": "stop",
        "attachments": [{
            "type": "file",
            "name": "a.txt",
            "size": 10,
            "hash": HASH_A,
            "attachmentOrder": 4,
            "extractedText": "extracted",
            "createdAt": 200
        }],
        "contentHash": "content-hash"
    }))
    .expect("valid canonical message DTO");

    let message = ChatMessage::from(dto);
    assert_eq!(message.id, "msg-1");
    assert_eq!(message.role, "assistant");
    assert_eq!(message.name.as_deref(), Some("Nova"));
    assert_eq!(message.content, "hello");
    assert_eq!(message.timestamp, 123);
    assert_eq!(message.updated_at, Some(124));
    assert_eq!(message.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(message.group_id.as_deref(), Some("group-1"));
    assert_eq!(message.topic_id.as_deref(), Some("topic-1"));
    assert_eq!(message.is_group_message, Some(true));
    assert_eq!(message.content_hash.as_deref(), Some("content-hash"));
    assert!(message.blocks.is_none());
    assert!(message.shell.is_none());

    let attachment = &message.attachments.as_ref().unwrap()[0];
    assert_eq!(attachment.hash.as_deref(), Some(HASH_A));
    assert_eq!(attachment.attachment_order, Some(4));
    assert_eq!(attachment.src, "");
    assert_eq!(attachment.internal_path, "");
    assert!(attachment.thumbnail_path.is_none());
    assert!(attachment.status.is_none());
}

#[test]
fn message_sync_dto_from_message_uses_explicit_update_clock_and_strips_local_fields() {
    let message = ChatMessage {
        id: "msg-1".to_string(),
        role: "user".to_string(),
        content: "hello".to_string(),
        timestamp: 123,
        updated_at: Some(456),
        attachments: None,
        ..ChatMessage::default()
    };

    let dto = MessageSyncDTO::from_message(&message, 999).expect("build message DTO");
    assert_eq!(dto.updated_at, 999);
    let value = serde_json::to_value(dto).expect("serialize message DTO");
    assert!(!value.as_object().unwrap().contains_key("avatarColor"));
    assert!(!value.as_object().unwrap().contains_key("blocks"));

    let legacy = MessageSyncDTO::from_message_legacy(&message).expect("build legacy DTO");
    assert_eq!(legacy.updated_at, 456);
}
