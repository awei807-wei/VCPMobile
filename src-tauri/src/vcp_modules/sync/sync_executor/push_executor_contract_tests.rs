use super::entities::{load_owner_push_version, owner_push_idempotency_key};
use super::message_store::{load_outbound_message_page, serialize_topic_messages};
use super::ndjson::parse_message_push_frames;
use super::tombstone::{message_tombstone_body_len, preflight_topic_messages};
use super::types::{BoundedJsonLine, MessageTombstone, MessageTombstoneRequest};
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::sync::sync_types::{AvatarPushResponse, EntityPushResponse, OwnerType};
use crate::vcp_modules::sync_dto::AttachmentSyncDTO;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;
use std::io::Write;

const MAX_SAFE_JSON_INTEGER: i64 = crate::vcp_modules::sync::sync_types::MAX_SAFE_TIMESTAMP;

fn topic(owner_type: &str, owner_id: &str, topic_id: &str) -> TopicKey {
    TopicKey::new(owner_type, owner_id, topic_id)
}

fn hash(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

#[test]
fn message_push_response_requires_full_composite_identity() {
    let expected = vec![
        topic("agent", "agent-a", "shared"),
        topic("group", "group-a", "shared"),
    ];
    let body = concat!(
        "{\"kind\":\"topic\",\"topicId\":\"shared\",\"ownerType\":\"agent\",\"ownerId\":\"agent-a\",\"ok\":true}\n",
        "{\"kind\":\"topic\",\"topicId\":\"shared\",\"ownerType\":\"group\",\"ownerId\":\"group-a\",\"ok\":true}\n"
    );
    let frames = parse_message_push_frames(body.as_bytes(), &expected).expect("identity response");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].outcome.topic, expected[0]);
    assert_eq!(frames[1].outcome.topic, expected[1]);
}

#[test]
fn message_push_response_rejects_duplicates_unknown_fields_and_legacy_errors() {
    let expected = vec![topic("agent", "agent-a", "topic")];
    let malformed = [
        br#"{"kind":"topic","topicId":"topic","ownerType":"agent","ownerId":"agent-a","ok":true,"ok":true}"#
            .as_slice(),
        br#"{"kind":"topic","topicId":"topic","ownerType":"agent","ownerId":"agent-a","ok":true,"legacy":true}"#
            .as_slice(),
        br#"{"kind":"topic","topicId":"topic","ownerType":"agent","ownerId":"agent-a","ok":false,"error":"legacy"}"#
            .as_slice(),
    ];
    for body in malformed {
        assert!(parse_message_push_frames(body, &expected).is_err());
    }
    let duplicate = concat!(
        "{\"kind\":\"topic\",\"topicId\":\"topic\",\"ownerType\":\"agent\",\"ownerId\":\"agent-a\",\"ok\":true}\n",
        "{\"kind\":\"topic\",\"topicId\":\"topic\",\"ownerType\":\"agent\",\"ownerId\":\"agent-a\",\"ok\":true}\n"
    );
    assert!(parse_message_push_frames(duplicate.as_bytes(), &expected).is_err());
}

#[test]
fn failed_message_push_preserves_structured_wire_error() {
    let expected = vec![topic("agent", "agent-a", "topic")];
    let body = serde_json::to_vec(&json!({
        "kind": "topic",
        "topicId": "topic",
        "ownerType": "agent",
        "ownerId": "agent-a",
        "ok": false,
        "error": {
            "code": "SYNC_OWNER_CONFLICT",
            "origin": "desktop_cds",
            "stage": "messages",
            "kind": "data",
            "retry": "manual",
            "message": "owner conflict",
            "failedTopicIds": ["topic"]
        }
    }))
    .expect("wire error");
    let frames = parse_message_push_frames(&body, &expected).expect("structured error");
    let encoded = frames[0].outcome.error.as_deref().expect("encoded error");
    assert!(encoded.starts_with("SYNC_WIRE_ERROR:"));
}

#[test]
fn stream_error_is_a_structured_wire_error() {
    let body = serde_json::to_vec(&json!({
        "kind": "streamError",
        "error": {
            "code": "SYNC_STREAM_FAILED",
            "origin": "desktop_plugin",
            "stage": "messages",
            "kind": "connection",
            "retry": "manual",
            "message": "stream failed",
            "failedTopicIds": []
        }
    }))
    .expect("stream error");
    assert!(parse_message_push_frames(&body, &[topic("agent", "a", "t")]).is_err());
}

#[test]
fn typed_entity_and_avatar_responses_reject_unknown_fields() {
    let entity: EntityPushResponse = serde_json::from_value(json!({
        "results": [{"entityType":"owner","ownerType":"agent","ownerId":"agent-a","ok":true}]
    }))
    .expect("entity response");
    entity.validate().expect("valid entity response");
    assert!(serde_json::from_value::<EntityPushResponse>(json!({
        "results": [{"entityType":"owner","ownerType":"agent","ownerId":"agent-a","ok":true,"legacy":true}]
    }))
    .is_err());
    let avatar: AvatarPushResponse = serde_json::from_value(json!({
        "ownerType": "agent", "ownerId": "agent-a", "ok": true
    }))
    .expect("avatar response");
    avatar.validate().expect("valid avatar response");
    assert!(serde_json::from_value::<AvatarPushResponse>(json!({
        "ownerType": "agent", "ownerId": "agent-a", "ok": true, "hash": hash('a')
    }))
    .is_err());
}

#[test]
fn canonical_attachment_metadata_normalizes_hash_and_preserves_order() {
    let attachment: AttachmentSyncDTO = serde_json::from_value(json!({
        "type": "image/png",
        "name": "one.png",
        "size": 3,
        "hash": hash('A'),
        "attachmentOrder": 2
    }))
    .expect("attachment metadata");
    assert_eq!(attachment.hash, hash('a'));
    assert_eq!(attachment.attachment_order, Some(2));
    assert!(serde_json::from_value::<AttachmentSyncDTO>(json!({
        "type": "image/png", "name": "bad", "size": 3, "hash": hash('a'), "attachmentOrder": -1
    }))
    .is_err());
}

#[test]
fn bounded_writer_never_returns_a_partial_overflow_write() {
    let mut line = BoundedJsonLine::new(32);
    line.write_all(br#"{"kind":"topic"}"#).expect("write");
    let bytes = line.into_bytes();
    serde_json::from_slice::<serde_json::Value>(&bytes).expect("valid bounded JSON");
    let mut tiny = BoundedJsonLine::new(3);
    assert!(tiny.write_all(b"four").is_err());
    assert!(tiny.bytes.is_empty());
}

#[tokio::test]
async fn outbound_snapshot_decodes_compressed_blob_and_keeps_order_and_tombstone() {
    let pool = test_pool().await;
    let compressed = ContentCompressor::compress("压缩正文").expect("compress body");
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "m-1",
            timestamp: 1,
            content: compressed,
            deleted_at: None,
        },
    )
    .await;
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "m-2",
            timestamp: 2,
            content: b"plain".to_vec(),
            deleted_at: None,
        },
    )
    .await;
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "m-old",
            timestamp: 3,
            content: b"gone".to_vec(),
            deleted_at: Some(9),
        },
    )
    .await;
    let key = topic("agent", "agent-a", "shared");
    let mut tx = pool.begin().await.expect("begin snapshot");
    let preflight = preflight_topic_messages(&mut tx, &key)
        .await
        .expect("preflight");
    assert_eq!(preflight.live_count, 2);
    assert_eq!(preflight.tombstone_count, 1);
    let page = load_outbound_message_page(&mut tx, &key, None)
        .await
        .expect("page");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].content, "压缩正文");
    assert_eq!(page[1].id, "m-2");
    let serialized = serialize_topic_messages(&mut tx, &key)
        .await
        .expect("serialize");
    let frame: serde_json::Value = serde_json::from_slice(&serialized.line).expect("frame");
    assert_eq!(frame["ownerId"], "agent-a");
    assert_eq!(frame["messages"][0]["id"], "m-1");
    assert_eq!(frame["deletedMessages"][0]["msgId"], "m-old");
    assert_eq!(frame["deletedMessages"][0]["deletedAt"], 9);
}

#[tokio::test]
async fn same_topic_id_in_another_owner_namespace_is_isolated() {
    let pool = test_pool().await;
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "m-a",
            timestamp: 1,
            content: b"agent".to_vec(),
            deleted_at: None,
        },
    )
    .await;
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "group",
            owner_id: "group-a",
            topic_id: "shared",
            msg_id: "m-g",
            timestamp: 1,
            content: b"group".to_vec(),
            deleted_at: None,
        },
    )
    .await;
    let mut tx = pool.begin().await.expect("begin snapshot");
    let agent = load_outbound_message_page(&mut tx, &topic("agent", "agent-a", "shared"), None)
        .await
        .expect("agent page");
    let group = load_outbound_message_page(&mut tx, &topic("group", "group-a", "shared"), None)
        .await
        .expect("group page");
    assert_eq!(agent[0].content, "agent");
    assert_eq!(group[0].content, "group");
}

#[tokio::test]
async fn outbound_message_snapshot_rejects_unsafe_timestamps_and_tombstones() {
    let pool = test_pool().await;
    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "unsafe-timestamp",
            timestamp: MAX_SAFE_JSON_INTEGER + 1,
            content: b"unsafe".to_vec(),
            deleted_at: None,
        },
    )
    .await;
    let key = topic("agent", "agent-a", "shared");
    let mut tx = pool.begin().await.expect("begin timestamp snapshot");
    assert!(load_outbound_message_page(&mut tx, &key, None)
        .await
        .is_err());
    tx.rollback().await.expect("rollback timestamp snapshot");
    sqlx::query("DELETE FROM messages WHERE msg_id = ?")
        .bind("unsafe-timestamp")
        .execute(&pool)
        .await
        .expect("remove timestamp fixture");

    insert_message(
        &pool,
        MessageFixture {
            owner_type: "agent",
            owner_id: "agent-a",
            topic_id: "shared",
            msg_id: "unsafe-tombstone",
            timestamp: 1,
            content: b"gone".to_vec(),
            deleted_at: Some(MAX_SAFE_JSON_INTEGER + 1),
        },
    )
    .await;
    let mut tx = pool.begin().await.expect("begin tombstone snapshot");
    assert!(serialize_topic_messages(&mut tx, &key).await.is_err());
}

#[test]
fn tombstone_request_has_only_current_wire_fields() {
    let tombstone = MessageTombstone {
        topic: topic("agent", "agent-a", "topic"),
        message_id: "m".to_string(),
        deleted_at: 7,
    };
    let request = MessageTombstoneRequest {
        msg_id: "m",
        deleted_at: 7,
    };
    assert_eq!(
        message_tombstone_body_len(&tombstone).unwrap(),
        serde_json::to_vec(&request).unwrap().len()
    );
}

#[test]
fn owner_push_idempotency_key_is_framed_and_binds_snapshot() {
    let config_hash = hash('a');
    let baseline = owner_push_idempotency_key(OwnerType::Agent, "owner-a", &config_hash, 100);
    assert_eq!(baseline.len(), 64);
    assert!(baseline.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(
        baseline,
        owner_push_idempotency_key(OwnerType::Group, "owner-a", &config_hash, 100)
    );
    assert_ne!(
        baseline,
        owner_push_idempotency_key(OwnerType::Agent, "owner-a", &config_hash, 101)
    );
    assert_ne!(
        owner_push_idempotency_key(OwnerType::Agent, "a", "bc", 100),
        owner_push_idempotency_key(OwnerType::Agent, "ab", "c", 100)
    );
}

#[tokio::test]
async fn owner_push_snapshot_rejects_stale_config_hash() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::raw_sql(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, config_hash TEXT NOT NULL,
            updated_at INTEGER NOT NULL, deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, config_hash TEXT NOT NULL,
            updated_at INTEGER NOT NULL, deleted_at INTEGER
         );
         INSERT INTO agents VALUES ('agent-a', 'agent-hash', 101, NULL);
         INSERT INTO groups VALUES ('group-a', 'group-hash', 202, NULL);",
    )
    .execute(&pool)
    .await
    .expect("create owner fixture");
    assert_eq!(
        load_owner_push_version(&pool, OwnerType::Agent, "agent-a", "agent-hash")
            .await
            .expect("matching snapshot"),
        101
    );
    let error = load_owner_push_version(&pool, OwnerType::Agent, "agent-a", "stale")
        .await
        .expect_err("stale snapshot");
    assert!(error.contains("changed while preparing"));
}

async fn test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open database");
    sqlx::raw_sql(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content BLOB NOT NULL, timestamp INTEGER NOT NULL, is_group_message INTEGER NOT NULL,
            group_id TEXT, finish_reason TEXT, updated_at INTEGER NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
            display_name TEXT NOT NULL, src TEXT, status TEXT, deleted_at INTEGER
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
            extracted_text TEXT, image_frames TEXT, created_at INTEGER
         );",
    )
    .execute(&pool)
    .await
    .expect("create fixture");
    sqlx::query("INSERT INTO topics(owner_type, owner_id, topic_id) VALUES ('agent','agent-a','shared'), ('group','group-a','shared')")
        .execute(&pool)
        .await
        .expect("insert topics");
    pool
}

struct MessageFixture<'a> {
    owner_type: &'a str,
    owner_id: &'a str,
    topic_id: &'a str,
    msg_id: &'a str,
    timestamp: i64,
    content: Vec<u8>,
    deleted_at: Option<i64>,
}

async fn insert_message(pool: &sqlx::SqlitePool, fixture: MessageFixture<'_>) {
    sqlx::query(
        "INSERT INTO messages(
            owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
            is_group_message, updated_at, deleted_at
         ) VALUES (?, ?, ?, ?, 'user', ?, ?, 0, ?, ?)",
    )
    .bind(fixture.owner_type)
    .bind(fixture.owner_id)
    .bind(fixture.topic_id)
    .bind(fixture.msg_id)
    .bind(fixture.content)
    .bind(fixture.timestamp)
    .bind(fixture.timestamp)
    .bind(fixture.deleted_at)
    .execute(pool)
    .await
    .expect("insert message");
}
