use super::attachments::validate_upload_response;
use super::avatar::validate_avatar_response;
use super::entities::validate_identity_response;
use super::message_store::load_outbound_message_page;
use super::ndjson::parse_message_push_frames;
use super::ndjson::record_message_frames;
use super::tombstone::{
    load_message_tombstones, message_tombstone_body_len, preflight_topic_messages,
    validate_message_tombstone_response,
};
use super::types::{
    canonical_sha256, BoundedJsonLine, MessagePushFrame, MessageTombstone, MessageTombstoneRequest,
    PushBatchResult, MESSAGE_PAGE_SIZE,
};
use crate::vcp_modules::message_repository::ContentCompressor;
use std::collections::{HashMap, HashSet};
use std::io::Write;

#[test]
fn message_push_response_rejects_duplicate_keys_and_unknown_fields() {
    let expected = vec!["topic-a".to_string()];
    for malformed in [
        br#"{"topicId":"topic-a","topicId":"topic-a","success":true,"neededAttachmentHashes":[]}"#
            .as_slice(),
        br#"{"topicId":"topic-a","success":true,"neededAttachmentHashes":[],"legacy":true}"#
            .as_slice(),
    ] {
        assert!(
            parse_message_push_frames(malformed, &expected).is_err(),
            "Wire 1.2 response parsing must fail closed"
        );
    }
}

#[tokio::test]
async fn outbound_loader_decodes_the_fork_compressed_content_contract() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE messages (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, role TEXT NOT NULL,
            name TEXT, agent_id TEXT, content BLOB NOT NULL, timestamp BIGINT NOT NULL,
            is_group_message INTEGER NOT NULL, group_id TEXT, finish_reason TEXT,
            content_hash TEXT NOT NULL, deleted_at BIGINT,
            PRIMARY KEY(topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            topic_id TEXT, msg_id TEXT, hash TEXT, attachment_order INTEGER,
            display_name TEXT, src TEXT, status TEXT, deleted_at BIGINT
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT, size BIGINT, internal_path TEXT,
            image_frames TEXT, thumbnail_path TEXT, created_at BIGINT
         );",
    )
    .execute(&pool)
    .await
    .expect("create fixture");
    let compressed = ContentCompressor::compress("fork 压缩正文").expect("compress body");
    sqlx::query(
        "INSERT INTO messages (
            msg_id, topic_id, role, content, timestamp, is_group_message, content_hash
         ) VALUES ('message-a', 'topic-a', 'user', ?, 1, 0, 'hash-a')",
    )
    .bind(compressed)
    .execute(&pool)
    .await
    .expect("insert message");

    let mut tx = pool.begin().await.expect("begin snapshot");
    let messages = load_outbound_message_page(&mut tx, "topic-a", None)
        .await
        .expect("load compressed message");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "fork 压缩正文");
}

#[test]
fn canonical_hash_is_lowercase_and_rejects_non_sha256_values() {
    assert_eq!(canonical_sha256(&"A".repeat(64)), Some("a".repeat(64)));
    assert_eq!(canonical_sha256(""), None);
    assert_eq!(canonical_sha256(&"g".repeat(64)), None);
    assert_eq!(canonical_sha256(&"a".repeat(63)), None);
}

#[test]
fn attachment_dependencies_are_recorded_per_successful_topic() {
    let hash = "a".repeat(64);
    let frames = vec![
        MessagePushFrame {
            outcome: PushBatchResult {
                topic_id: "topic-a".to_string(),
                success: true,
                error: None,
            },
            needed_attachment_hashes: vec![hash.clone()],
        },
        MessagePushFrame {
            outcome: PushBatchResult {
                topic_id: "topic-b".to_string(),
                success: false,
                error: Some("rejected".to_string()),
            },
            needed_attachment_hashes: Vec::new(),
        },
    ];
    let mut results = Vec::new();
    let mut dependencies = HashMap::new();
    record_message_frames(frames, &mut results, &mut dependencies);
    assert_eq!(results.len(), 2);
    assert_eq!(
        dependencies.get(&hash),
        Some(&HashSet::from(["topic-a".to_string()]))
    );
}

#[test]
fn message_push_result_requires_explicit_attachment_hash_array() {
    let expected = vec!["topic".to_string()];
    let missing = br#"{"topicId":"topic","success":true}"#;
    let error = match parse_message_push_frames(missing, &expected) {
        Err(error) => error,
        Ok(_) => panic!("legacy result without neededAttachmentHashes must be rejected"),
    };
    assert!(error.contains("explicit array"));
    let valid = br#"{"topicId":"topic","success":true,"neededAttachmentHashes":[]}"#;
    let frames = parse_message_push_frames(valid, &expected).expect("hard-cut result");
    assert_eq!(frames.len(), 1);
    assert!(frames[0].needed_attachment_hashes.is_empty());
}

#[test]
fn failed_message_push_preserves_wire_error_and_rejects_legacy_strings() {
    let expected = vec!["topic".to_string()];
    let valid = serde_json::to_vec(&serde_json::json!({
        "topicId":"topic",
        "success":false,
        "neededAttachmentHashes":[],
        "error":{
            "code":"SYNC_OWNER_CONFLICT",
            "origin":"desktop_cds",
            "stage":"messages",
            "kind":"data",
            "retry":"manual",
            "message":"owner conflict",
            "failedTopicIds":["topic"]
        }
    }))
    .expect("serialize result");
    let frames = parse_message_push_frames(&valid, &expected).expect("Wire 1.2 result");
    assert_eq!(
        crate::vcp_modules::sync_error::decode_wire_sync_error(
            frames[0].outcome.error.as_deref().expect("encoded error")
        )
        .expect("wire error")
        .code,
        "SYNC_OWNER_CONFLICT"
    );
    let legacy = serde_json::to_vec(&serde_json::json!({
        "topicId":"topic",
        "success":false,
        "neededAttachmentHashes":[],
        "error":"legacy"
    }))
    .expect("serialize legacy");
    assert!(parse_message_push_frames(&legacy, &expected).is_err());
    let contradictory = serde_json::to_vec(&serde_json::json!({
        "topicId":"topic",
        "success":true,
        "neededAttachmentHashes":[],
        "error":{
            "code":"SYNC_OWNER_CONFLICT",
            "origin":"desktop_cds",
            "stage":"messages",
            "kind":"data",
            "retry":"manual",
            "message":"owner conflict",
            "failedTopicIds":["topic"]
        }
    }))
    .expect("serialize contradictory result");
    assert!(parse_message_push_frames(&contradictory, &expected).is_err());
}

#[test]
fn bounded_writer_emits_valid_ndjson_and_rejects_overflow() {
    let mut line = BoundedJsonLine::new(64);
    line.write_all(br#"{"topicId":"#).unwrap();
    serde_json::to_writer(&mut line, "topic").unwrap();
    line.write_all(b",\"messages\":[]}\n").unwrap();
    let bytes = line.into_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes[..bytes.len() - 1])
        .expect("bounded line must remain valid JSON");
    let mut tiny = BoundedJsonLine::new(3);
    assert!(tiny.write_all(b"four").is_err());
    assert!(tiny.bytes.is_empty());
}

#[tokio::test]
async fn outbound_message_loader_uses_stable_bounded_pages() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::query(
        "CREATE TABLE messages (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, role TEXT NOT NULL,
            name TEXT, agent_id TEXT, content TEXT NOT NULL, timestamp BIGINT NOT NULL,
            is_group_message INTEGER NOT NULL, group_id TEXT, finish_reason TEXT,
            content_hash TEXT NOT NULL, deleted_at BIGINT,
            PRIMARY KEY(topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            topic_id TEXT, msg_id TEXT, hash TEXT, attachment_order INTEGER,
            display_name TEXT, src TEXT, status TEXT, deleted_at BIGINT
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT, size BIGINT, internal_path TEXT,
            image_frames TEXT, thumbnail_path TEXT, created_at BIGINT
         );",
    )
    .execute(&pool)
    .await
    .expect("create fixture");
    for index in 0..=MESSAGE_PAGE_SIZE {
        sqlx::query(
            "INSERT INTO messages (
                msg_id, topic_id, role, content, timestamp, is_group_message, content_hash
             ) VALUES (?, 'topic', 'user', 'body', ?, 0, 'hash')",
        )
        .bind(format!("message-{index:03}"))
        .bind(index as i64)
        .execute(&pool)
        .await
        .expect("insert message");
    }
    sqlx::query(
        "INSERT INTO messages (
            msg_id, topic_id, role, content, timestamp, is_group_message,
            content_hash, deleted_at
         ) VALUES ('message-deleted', 'topic', 'user', 'gone', 200, 0, 'DELETED', 1234)",
    )
    .execute(&pool)
    .await
    .expect("insert tombstone");
    let mut read_tx = pool.begin().await.expect("begin snapshot");
    let preflight = preflight_topic_messages(&mut read_tx, "topic")
        .await
        .expect("preflight");
    assert_eq!(preflight.live_count, MESSAGE_PAGE_SIZE + 1);
    assert_eq!(preflight.tombstone_count, 1);
    let tombstones = load_message_tombstones(&mut read_tx, "topic", 1)
        .await
        .expect("load tombstone");
    assert_eq!(tombstones[0].message_id, "message-deleted");
    assert_eq!(tombstones[0].deleted_at, 1234);
    let first = load_outbound_message_page(&mut read_tx, "topic", None)
        .await
        .expect("first page");
    assert_eq!(first.len(), MESSAGE_PAGE_SIZE);
    assert_eq!(first.first().unwrap().id, "message-000");
    assert_eq!(first.last().unwrap().id, "message-099");
    let second = load_outbound_message_page(&mut read_tx, "topic", Some((99, "message-099")))
        .await
        .expect("second page");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, "message-100");
}

#[test]
fn tombstone_request_is_exactly_budgeted_and_response_identity_is_checked() {
    let tombstone = MessageTombstone {
        topic_id: "topic-a".to_string(),
        message_id: "message-a".to_string(),
        deleted_at: 1234,
    };
    let request = MessageTombstoneRequest {
        topic_id: &tombstone.topic_id,
        msg_id: &tombstone.message_id,
        deleted_at: tombstone.deleted_at,
    };
    assert_eq!(
        message_tombstone_body_len(&tombstone).expect("count body"),
        serde_json::to_vec(&request).expect("serialize body").len()
    );
    validate_message_tombstone_response(
        &serde_json::json!({
            "success": true,
            "topicId": "topic-a",
            "msgId": "message-a"
        }),
        &tombstone,
    )
    .expect("matching identity");
    assert!(validate_message_tombstone_response(
        &serde_json::json!({
            "success": true,
            "topicId": "topic-b",
            "msgId": "message-a"
        }),
        &tombstone,
    )
    .is_err());
    for malformed in [
        serde_json::json!({ "success": true, "msgId": "message-a" }),
        serde_json::json!({ "success": true, "topicId": "topic-a" }),
        serde_json::json!({ "success": true, "topicId": 1, "msgId": "message-a" }),
        serde_json::json!({ "success": true, "topicId": "topic-a", "msgId": 1 }),
    ] {
        assert!(validate_message_tombstone_response(&malformed, &tombstone).is_err());
    }
}

#[test]
fn stream_error_requires_the_exact_wire_wrapper() {
    let expected = vec!["topic-a".to_string()];
    let valid = serde_json::json!({
        "_stream_error": {
            "code":"SYNC_PROTOCOL_VIOLATION",
            "origin":"desktop_cds",
            "stage":"messages",
            "kind":"protocol",
            "retry":"manual",
            "message":"bad frame",
            "failedTopicIds":["topic-a"]
        }
    });
    let valid = serde_json::to_vec(&valid).unwrap();
    assert!(parse_message_push_frames(&valid, &expected).is_err());
    let extra = serde_json::json!({
        "_stream_error": valid,
        "topicId":"topic-a"
    });
    assert!(parse_message_push_frames(&serde_json::to_vec(&extra).unwrap(), &expected).is_err());
}

#[test]
fn entity_avatar_and_attachment_responses_require_exact_identity() {
    let entity = serde_json::json!({ "success": true, "id": "agent-a" });
    validate_identity_response(&entity, "agent-a", "Push agent").expect("entity identity");
    assert!(validate_identity_response(
        &serde_json::json!({ "success": true, "id": "other" }),
        "agent-a",
        "Push agent"
    )
    .is_err());
    assert!(validate_identity_response(
        &serde_json::json!({ "success": true, "id": "agent-a", "legacy": true }),
        "agent-a",
        "Push agent"
    )
    .is_err());

    let avatar = serde_json::json!({ "success": true, "id": "agent-a" });
    validate_avatar_response(&avatar, "agent-a", "agent").expect("avatar identity");
    let hash = "a".repeat(64);
    let attachment = serde_json::json!({ "success": true, "hash": hash.to_ascii_uppercase() });
    validate_upload_response(&attachment, &hash).expect("canonical attachment identity");
    assert!(validate_upload_response(
        &serde_json::json!({ "success": true, "hash": hash, "src": "/private/path" }),
        &"a".repeat(64)
    )
    .is_err());
}
