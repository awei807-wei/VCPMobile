use super::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, AttachmentSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
    MessageSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::{MessageLiveState, MessageVersionState};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use rusqlite::Connection;
use std::collections::BTreeMap;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn agent_dto(name: &str) -> AgentSyncDTO {
    AgentSyncDTO {
        name: name.to_string(),
        system_prompt: "system".to_string(),
        model: "model".to_string(),
        temperature: 1.0,
        context_token_limit: 1024,
        max_output_tokens: 256,
        stream_output: true,
    }
}

fn group_dto(name: &str) -> GroupSyncDTO {
    GroupSyncDTO {
        name: name.to_string(),
        members: Vec::new(),
        mode: "round".to_string(),
        member_tags: None,
        group_prompt: None,
        invite_prompt: None,
        use_unified_model: false,
        unified_model: None,
        tag_match_mode: None,
        created_at: 1,
    }
}

fn agent_topic(topic_id: &str, owner_id: &str) -> AgentTopicSyncDTO {
    AgentTopicSyncDTO {
        id: topic_id.to_string(),
        name: "Agent topic".to_string(),
        created_at: 1,
        locked: true,
        unread: false,
        owner_id: owner_id.to_string(),
    }
}

fn group_topic(topic_id: &str, owner_id: &str) -> GroupTopicSyncDTO {
    GroupTopicSyncDTO {
        id: topic_id.to_string(),
        name: "Group topic".to_string(),
        created_at: 1,
        owner_id: owner_id.to_string(),
    }
}

fn canonical_message(
    id: &str,
    topic_id: &str,
    content: &str,
    attachment_hash: &str,
) -> MessageSyncDTO {
    MessageSyncDTO {
        id: id.to_string(),
        role: "user".to_string(),
        name: None,
        content: content.to_string(),
        timestamp: 10,
        updated_at: 11,
        is_thinking: None,
        agent_id: None,
        group_id: None,
        topic_id: Some(topic_id.to_string()),
        is_group_message: Some(false),
        finish_reason: None,
        attachments: Some(vec![AttachmentSyncDTO {
            r#type: "text/plain".to_string(),
            name: "canonical.txt".to_string(),
            size: 4,
            hash: attachment_hash.to_string(),
            attachment_order: Some(2),
            extracted_text: None,
            image_frames: None,
            created_at: Some(10),
            status: None,
        }]),
        content_hash: None,
    }
}

fn setup_schema(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, name TEXT, system_prompt TEXT, model TEXT,
                temperature REAL, context_token_limit INTEGER, max_output_tokens INTEGER,
                stream_output INTEGER, config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '', updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, name TEXT, mode TEXT, group_prompt TEXT,
                invite_prompt TEXT, use_unified_model INTEGER, unified_model TEXT,
                tag_match_mode TEXT, created_at INTEGER, config_hash TEXT NOT NULL DEFAULT '',
                content_hash TEXT NOT NULL DEFAULT '', updated_at INTEGER, deleted_at INTEGER
             );
             CREATE TABLE group_members (
                group_id TEXT, agent_id TEXT, member_tag TEXT, sort_order INTEGER, updated_at INTEGER
             );
             CREATE TABLE avatars (
                owner_type TEXT, owner_id TEXT, avatar_hash TEXT, mime_type TEXT,
                image_data BLOB, dominant_color TEXT, updated_at INTEGER, deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id)
             );
             CREATE TABLE topics (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                title TEXT, created_at INTEGER, locked INTEGER, unread INTEGER,
                updated_at INTEGER,
                last_message_updated_at INTEGER NOT NULL DEFAULT 0,
                unread_count INTEGER NOT NULL DEFAULT 0, msg_count INTEGER NOT NULL DEFAULT 0,
                config_hash TEXT NOT NULL DEFAULT '', content_hash TEXT NOT NULL DEFAULT '',
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT,
                agent_id TEXT, content BLOB NOT NULL, timestamp INTEGER,
                is_group_message INTEGER, group_id TEXT, finish_reason TEXT,
                content_hash TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER,
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE TABLE render_cache (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, render_content BLOB, updated_at INTEGER,
                content_hash TEXT NOT NULL DEFAULT '', renderer_schema_version INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE TABLE messages_fts (
                msg_id TEXT, topic_id TEXT, content TEXT, owner_type TEXT, owner_id TEXT
             );
             CREATE TABLE attachments (
                hash TEXT PRIMARY KEY, mime_type TEXT, size INTEGER, internal_path TEXT,
                extracted_text TEXT, image_frames TEXT, thumbnail_path TEXT,
                created_at INTEGER, updated_at INTEGER
             );
             CREATE TABLE message_attachments (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT, hash TEXT, attachment_order INTEGER,
                display_name TEXT, src TEXT, status TEXT, created_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id, attachment_order)
             );
             CREATE TABLE active_generations (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
             );",
        )
        .expect("contract schema should be created");
}

fn seed_entities(connection: &Connection) {
    connection
        .execute_batch(
            "INSERT INTO agents (agent_id, name, deleted_at) VALUES
                ('agent-live', 'live agent', NULL),
                ('agent-other', 'other agent', NULL),
                ('agent-deleted', 'deleted agent', 9);
             INSERT INTO groups (group_id, name, deleted_at) VALUES
                ('group-live', 'live group', NULL),
                ('group-deleted', 'deleted group', 9);
             INSERT INTO group_members (group_id, agent_id, member_tag, sort_order, updated_at)
                VALUES ('group-deleted', 'old-agent', 'old', 0, 1);
             INSERT INTO topics
                (owner_type, owner_id, topic_id, title, created_at, locked, unread, updated_at, deleted_at)
                VALUES
                ('agent', 'agent-live', 'topic-live', 'live topic', 1, 1, 0, 1, NULL),
                ('agent', 'agent-live', 'topic-deleted', 'deleted topic', 1, 1, 0, 1, 9),
                ('group', 'group-live', 'topic-group', 'group topic', 1, 1, 0, 1, NULL);
             INSERT INTO avatars
                (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at, deleted_at)
                VALUES ('agent', 'agent-deleted', 'old', 'image/png', X'09', 1, 9);
            INSERT INTO messages
                (owner_type, owner_id, topic_id, msg_id, role, content, timestamp, created_at, updated_at, deleted_at)
                VALUES ('agent', 'agent-live', 'topic-live', 'message-deleted', 'user', X'5B64656C657465645D', 1, 1, 1, 9);
             INSERT INTO render_cache (owner_type, owner_id, topic_id, msg_id, render_content, updated_at)
                VALUES ('agent', 'agent-live', 'topic-live', 'message-deleted', X'09', 1);
             INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
                VALUES ('message-deleted', 'topic-live', 'deleted index', 'agent', 'agent-live');
             INSERT INTO message_attachments
                (owner_type, owner_id, topic_id, msg_id, hash, attachment_order, display_name, src, status, created_at)
                VALUES ('agent', 'agent-live', 'topic-live', 'message-deleted', 'old-hash', 0, 'old', '/old', 'ready', 1);",
        )
        .expect("contract entities should be seeded");
    let compressed = ContentCompressor::compress("[deleted]").expect("compress tombstone body");
    connection
        .execute(
            "UPDATE messages SET content = ? WHERE owner_type = 'agent' AND owner_id = 'agent-live' AND topic_id = 'topic-live' AND msg_id = 'message-deleted'",
            rusqlite::params![compressed],
        )
        .expect("store compressed tombstone body");
}

#[test]
fn entity_writes_validate_ids_parents_and_tombstones() {
    let mut connection = Connection::open_in_memory().expect("open test database");
    setup_schema(&connection);
    seed_entities(&connection);
    let tx = connection.transaction().expect("begin transaction");

    assert!(DbWriteQueue::rusqlite_upsert_agent(&tx, "", &agent_dto("bad")).is_err());
    assert!(DbWriteQueue::rusqlite_upsert_group(&tx, "", &group_dto("bad")).is_err());
    assert!(
        DbWriteQueue::rusqlite_upsert_agent(&tx, "agent-deleted", &agent_dto("stale"),).is_err()
    );
    assert!(
        DbWriteQueue::rusqlite_upsert_group(&tx, "group-deleted", &group_dto("stale"),).is_err()
    );
    assert!(DbWriteQueue::rusqlite_upsert_avatar(&tx, "agent", "", &[1]).is_err());
    assert!(DbWriteQueue::rusqlite_upsert_avatar(&tx, "agent", "missing", &[1]).is_err());
    assert!(DbWriteQueue::rusqlite_upsert_avatar(&tx, "agent", "agent-deleted", &[1]).is_err());

    DbWriteQueue::rusqlite_upsert_agent_topic(
        &tx,
        "topic-live",
        &agent_topic("topic-live", "agent-other"),
    )
    .expect("same topic id may be owned independently");
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM topics WHERE topic_id = 'topic-live'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        2
    );
    let owner_mismatch = DbWriteQueue::rusqlite_upsert_agent_topic_for_key(
        &tx,
        &TopicKey::new("agent", "agent-other", "topic-live"),
        &agent_topic("topic-live", "agent-live"),
    )
    .expect_err("DTO owner mismatch must fail closed");
    assert!(owner_mismatch
        .to_string()
        .contains("matching non-empty composite topic identity"));
    assert!(DbWriteQueue::rusqlite_upsert_agent_topic(
        &tx,
        "topic-deleted",
        &agent_topic("topic-deleted", "agent-live"),
    )
    .is_err());
    assert!(DbWriteQueue::rusqlite_upsert_agent_topic(
        &tx,
        "new-topic",
        &agent_topic("new-topic", "missing"),
    )
    .is_err());
    DbWriteQueue::rusqlite_upsert_group_topic(
        &tx,
        "topic-live",
        &group_topic("topic-live", "group-live"),
    )
    .expect("group and agent namespaces may reuse a topic id");
    assert!(DbWriteQueue::rusqlite_upsert_group_topic(
        &tx,
        "new-group-topic",
        &group_topic("new-group-topic", "group-deleted"),
    )
    .is_err());
    assert!(
        DbWriteQueue::rusqlite_upsert_group_topic(&tx, "", &group_topic("", "group-live"),)
            .is_err()
    );

    assert_eq!(
        tx.query_row(
            "SELECT name FROM agents WHERE agent_id = 'agent-deleted'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "deleted agent"
    );
    assert_eq!(
        tx.query_row(
            "SELECT agent_id FROM group_members WHERE group_id = 'group-deleted'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "old-agent"
    );
}

#[test]
fn message_batch_validates_vectors_identity_and_tombstones() {
    let mut connection = Connection::open_in_memory().expect("open test database");
    setup_schema(&connection);
    seed_entities(&connection);
    let tx = connection.transaction().expect("begin transaction");

    let message = ChatMessage {
        id: "message-live".to_string(),
        role: "user".to_string(),
        content: "live body".to_string(),
        topic_id: Some("topic-live".to_string()),
        timestamp: 2,
        ..Default::default()
    };
    assert!(DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        vec![message.clone()],
        Vec::new(),
        vec![vec![1]],
        vec!["hash".to_string()],
    )
    .is_err());
    assert!(DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "missing-topic",
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .is_err());

    let mut empty_id = message.clone();
    empty_id.id.clear();
    assert!(DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        vec![empty_id],
        vec![vec![1]],
        vec![vec![1]],
        vec!["hash".to_string()],
    )
    .is_err());
    let mut conflicting_topic = message.clone();
    conflicting_topic.topic_id = Some("topic-group".to_string());
    assert!(DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        vec![conflicting_topic],
        vec![vec![1]],
        vec![vec![1]],
        vec!["hash".to_string()],
    )
    .is_err());
    let duplicate = vec![message.clone(), message.clone()];
    assert!(DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        duplicate,
        vec![vec![1], vec![2]],
        vec![Vec::new(), Vec::new()],
        vec!["one".to_string(), "two".to_string()],
    )
    .is_err());

    let tombstone = ChatMessage {
        id: "message-deleted".to_string(),
        role: "assistant".to_string(),
        content: "stale remote body".to_string(),
        topic_id: None,
        timestamp: 9,
        attachments: Some(vec![Attachment {
            name: "stale.txt".to_string(),
            src: "/stale".to_string(),
            hash: Some("stale-hash".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };
    DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        vec![tombstone],
        vec![vec![1, 2, 3]],
        vec![vec![4, 5]],
        vec!["stale-content-hash".to_string()],
    )
    .expect("tombstone should be skipped without side-table writes");
    let stored_content: Vec<u8> = tx
        .query_row(
            "SELECT content FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'message-deleted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ContentCompressor::decompress(&stored_content).expect("tombstone body is compressed"),
        "[deleted]"
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM render_cache", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM messages_fts", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM message_attachments", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );

    let live = ChatMessage {
        id: "message-live".to_string(),
        role: "user".to_string(),
        content: "live body".to_string(),
        topic_id: Some("topic-live".to_string()),
        timestamp: 10,
        attachments: Some(vec![Attachment {
            r#type: "text/plain".to_string(),
            name: "live.txt".to_string(),
            src: "/live".to_string(),
            size: 4,
            hash: Some(HASH_A.to_string()),
            status: Some("ready".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    };
    DbWriteQueue::rusqlite_upsert_messages_batch(
        &tx,
        "topic-live",
        vec![live],
        vec![vec![0x01, 0x02]],
        vec![vec![0x03]],
        vec!["live-content-hash".to_string()],
    )
    .expect("live message should preserve compressed BLOB contract");
    let blob: Vec<u8> = tx
        .query_row(
            "SELECT content FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'message-live'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(blob, vec![0x01, 0x02]);
    assert_eq!(
        tx.query_row(
            "SELECT content FROM messages_fts
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'message-live'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "live body"
    );
    assert_eq!(
        tx.query_row(
            "SELECT hash FROM message_attachments
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'message-live'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        HASH_A
    );
}

#[test]
fn canonical_message_writes_and_deletes_are_owner_scoped() {
    let mut connection = Connection::open_in_memory().expect("open test database");
    setup_schema(&connection);
    seed_entities(&connection);
    let tx = connection.transaction().expect("begin transaction");
    tx.execute(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, title, created_at, locked, unread, updated_at)
         VALUES ('agent', 'agent-other', 'topic-live', 'other topic', 1, 1, 0, 1)",
        [],
    )
    .expect("second owner topic should be insertable");

    let key_a = TopicKey::new("agent", "agent-live", "topic-live");
    let key_b = TopicKey::new("agent", "agent-other", "topic-live");
    let message_a = canonical_message("same-message", "topic-live", "owner A", HASH_A);
    let message_b = canonical_message("same-message", "topic-live", "owner B", HASH_B);
    DbWriteQueue::rusqlite_upsert_messages_batch_for_key(
        &tx,
        &key_a,
        vec![message_a.clone()],
        vec![vec![0xA1]],
        vec![vec![0xA2]],
    )
    .expect("owner A canonical write should succeed");
    DbWriteQueue::rusqlite_upsert_messages_batch_for_key(
        &tx,
        &key_b,
        vec![message_b.clone()],
        vec![vec![0xB1]],
        vec![vec![0xB2]],
    )
    .expect("owner B canonical write should succeed");

    let row_count = tx
        .query_row(
            "SELECT COUNT(*) FROM messages
             WHERE topic_id = 'topic-live' AND msg_id = 'same-message'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(row_count, 2);
    for (key, expected_content, expected_hash) in
        [(&key_a, "owner A", HASH_A), (&key_b, "owner B", HASH_B)]
    {
        let (content, content_hash): (Vec<u8>, String) = tx
            .query_row(
                "SELECT content, content_hash FROM messages
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
                rusqlite::params![
                    &key.owner_type,
                    &key.owner_id,
                    &key.topic_id,
                    "same-message"
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            content,
            vec![if expected_content == "owner A" {
                0xA1
            } else {
                0xB1
            }]
        );
        let expected_dto = if expected_content == "owner A" {
            &message_a
        } else {
            &message_b
        };
        assert_eq!(
            content_hash,
            HashAggregator::compute_message_fingerprint_for_dto(expected_dto)
        );
        let relation: (String, Option<String>, Option<String>, i32) = tx
            .query_row(
                "SELECT hash, src, status, attachment_order FROM message_attachments
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
                rusqlite::params![
                    &key.owner_type,
                    &key.owner_id,
                    &key.topic_id,
                    "same-message"
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(relation, (expected_hash.to_string(), None, None, 2));
    }

    let message_key_a = MessageKey::new(key_a.clone(), "same-message");
    assert!(DbWriteQueue::rusqlite_delete_message_for_key(&tx, &message_key_a, 20).unwrap());
    assert_eq!(
        tx.query_row(
            "SELECT deleted_at FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'same-message'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap(),
        Some(20)
    );
    assert_eq!(
        tx.query_row(
            "SELECT deleted_at FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-other'
               AND topic_id = 'topic-live' AND msg_id = 'same-message'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap(),
        None
    );
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM message_attachments
             WHERE owner_type = 'agent' AND owner_id = 'agent-other'
               AND topic_id = 'topic-live' AND msg_id = 'same-message'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );

    assert!(DbWriteQueue::rusqlite_delete_topic_for_key(&tx, &key_b, 21).unwrap());
    assert_eq!(
        tx.query_row(
            "SELECT deleted_at FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-other' AND topic_id = 'topic-live'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap(),
        Some(21)
    );
    assert!(DbWriteQueue::rusqlite_delete_topic_for_key(&tx, &key_a, 22).unwrap());
    assert_eq!(
        tx.query_row(
            "SELECT deleted_at FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-live' AND topic_id = 'topic-live'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap(),
        Some(22)
    );
}

#[test]
fn canonical_pull_rejects_a_local_edit_after_the_phase3_snapshot() {
    let mut connection = Connection::open_in_memory().expect("open test database");
    setup_schema(&connection);
    seed_entities(&connection);
    let tx = connection.transaction().expect("begin transaction");
    let key = TopicKey::new("agent", "agent-live", "topic-live");
    let original = canonical_message("raced-message", "topic-live", "snapshot", HASH_A);
    let snapshot_hash = HashAggregator::compute_message_fingerprint_for_dto(&original);
    DbWriteQueue::rusqlite_upsert_messages_batch_for_key(
        &tx,
        &key,
        vec![original],
        vec![vec![0xA1]],
        vec![Vec::new()],
    )
    .expect("snapshot message should be stored");
    let expected = BTreeMap::from([(
        "raced-message".to_string(),
        Some(MessageVersionState::Live(MessageLiveState {
            message_hash: snapshot_hash,
            updated_at: 11,
        })),
    )]);

    tx.execute(
        "UPDATE messages SET content = X'CAFE', content_hash = ?, updated_at = 12
         WHERE owner_type = 'agent' AND owner_id = 'agent-live'
           AND topic_id = 'topic-live' AND msg_id = 'raced-message'",
        [HASH_B],
    )
    .expect("simulate a local edit after snapshot");
    let mut remote = canonical_message("raced-message", "topic-live", "stale remote", HASH_A);
    remote.updated_at = 13;
    let error = DbWriteQueue::rusqlite_upsert_messages_batch_for_key_if_unchanged(
        &tx,
        &key,
        vec![remote],
        vec![vec![0xB1]],
        vec![Vec::new()],
        Some(&expected),
    )
    .expect_err("stale pull must not overwrite the local edit");
    assert!(error.to_string().contains("SYNC_SNAPSHOT_STALE"));
    let stored: (Vec<u8>, String, i64) = tx
        .query_row(
            "SELECT content, content_hash, updated_at FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-live'
               AND topic_id = 'topic-live' AND msg_id = 'raced-message'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("local edit must remain stored");
    assert_eq!(stored, (vec![0xCA, 0xFE], HASH_B.to_string(), 12));
}

#[test]
fn owner_hash_bubbling_rejects_missing_or_deleted_owners() {
    let mut connection = Connection::open_in_memory().expect("open test database");
    setup_schema(&connection);
    seed_entities(&connection);
    let tx = connection.transaction().expect("begin transaction");

    assert!(DbWriteQueue::rusqlite_bubble_agent_hash(&tx, "").is_err());
    assert!(DbWriteQueue::rusqlite_bubble_agent_hash(&tx, "missing-agent").is_err());
    assert!(DbWriteQueue::rusqlite_bubble_agent_hash(&tx, "agent-deleted").is_err());
    assert!(DbWriteQueue::rusqlite_bubble_group_hash(&tx, "missing-group").is_err());
    assert!(DbWriteQueue::rusqlite_bubble_group_hash(&tx, "group-deleted").is_err());

    tx.execute(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, title, created_at, locked, unread, updated_at)
         VALUES ('agent', 'missing-agent', 'topic-orphan', 'orphan', 1, 1, 0, 1)",
        [],
    )
    .unwrap();
    assert!(DbWriteQueue::rusqlite_bubble_topic_hash(&tx, "topic-orphan").is_err());
    assert!(DbWriteQueue::rusqlite_bubble_topic_hash(&tx, "missing-topic").is_err());
    DbWriteQueue::rusqlite_bubble_agent_hash(&tx, "agent-live")
        .expect("live owner should bubble successfully");
}

#[tokio::test]
async fn submit_and_flush_propagate_worker_errors() {
    let path = std::env::temp_dir().join(format!(
        "vcp-mobile-write-queue-error-{}-{}.sqlite",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open unused pool");
    let mut queue = DbWriteQueue::new(pool, path.clone());
    queue
        .submit(DbWriteTask::Agent {
            id: "agent".to_string(),
            dto: agent_dto("agent"),
        })
        .await
        .expect("task should enter queue");
    let error = queue
        .flush()
        .await
        .expect_err("worker schema error must reach flush");
    assert!(error.contains("rusqlite execution error"));
    queue
        .flush()
        .await
        .expect("pending error should be consumed once");

    if let Some(worker) = queue._worker.take() {
        drop(queue);
        worker.await.expect("worker should stop cleanly");
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}
