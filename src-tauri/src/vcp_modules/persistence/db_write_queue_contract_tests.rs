use super::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use rusqlite::Connection;

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
                topic_id TEXT PRIMARY KEY, title TEXT, owner_id TEXT, owner_type TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, updated_at INTEGER,
                config_hash TEXT NOT NULL DEFAULT '', content_hash TEXT NOT NULL DEFAULT '',
                deleted_at INTEGER
             );
             CREATE TABLE messages (
                msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, role TEXT, name TEXT,
                agent_id TEXT, content BLOB NOT NULL, timestamp INTEGER,
                is_group_message INTEGER, group_id TEXT, finish_reason TEXT,
                content_hash TEXT NOT NULL DEFAULT '', created_at INTEGER, updated_at INTEGER,
                deleted_at INTEGER, PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE render_cache (
                topic_id TEXT, msg_id TEXT, render_content BLOB, updated_at INTEGER,
                PRIMARY KEY(topic_id, msg_id)
             );
             CREATE TABLE messages_fts (msg_id TEXT, topic_id TEXT, content TEXT);
             CREATE TABLE attachments (
                hash TEXT PRIMARY KEY, mime_type TEXT, size INTEGER, internal_path TEXT,
                extracted_text TEXT, image_frames TEXT, thumbnail_path TEXT,
                created_at INTEGER, updated_at INTEGER
             );
             CREATE TABLE message_attachments (
                topic_id TEXT, msg_id TEXT, hash TEXT, attachment_order INTEGER,
                display_name TEXT, src TEXT, status TEXT, created_at INTEGER,
                PRIMARY KEY(topic_id, msg_id, attachment_order)
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
                (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at, deleted_at)
                VALUES
                ('topic-live', 'live topic', 'agent-live', 'agent', 1, 1, 0, 1, NULL),
                ('topic-deleted', 'deleted topic', 'agent-live', 'agent', 1, 1, 0, 1, 9),
                ('topic-group', 'group topic', 'group-live', 'group', 1, 1, 0, 1, NULL);
             INSERT INTO avatars
                (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at, deleted_at)
                VALUES ('agent', 'agent-deleted', 'old', 'image/png', X'09', 1, 9);
             INSERT INTO messages
                (msg_id, topic_id, role, content, timestamp, created_at, updated_at, deleted_at)
                VALUES ('message-deleted', 'topic-live', 'user', X'5B64656C657465645D', 1, 1, 1, 9);
             INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at)
                VALUES ('topic-live', 'message-deleted', X'09', 1);
             INSERT INTO messages_fts (msg_id, topic_id, content)
                VALUES ('message-deleted', 'topic-live', 'deleted index');
             INSERT INTO message_attachments
                (topic_id, msg_id, hash, attachment_order, display_name, src, status, created_at)
                VALUES ('topic-live', 'message-deleted', 'old-hash', 0, 'old', '/old', 'ready', 1);",
        )
        .expect("contract entities should be seeded");
    let compressed = ContentCompressor::compress("[deleted]").expect("compress tombstone body");
    connection
        .execute(
            "UPDATE messages SET content = ? WHERE topic_id = 'topic-live' AND msg_id = 'message-deleted'",
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

    let agent_conflict = DbWriteQueue::rusqlite_upsert_agent_topic(
        &tx,
        "topic-live",
        &agent_topic("topic-live", "agent-other"),
    )
    .expect_err("topic owner conflict must fail closed");
    assert!(agent_conflict.to_string().contains("owner conflicts"));
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
    assert!(DbWriteQueue::rusqlite_upsert_group_topic(
        &tx,
        "topic-live",
        &group_topic("topic-live", "group-live"),
    )
    .is_err());
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
            "SELECT content FROM messages WHERE topic_id = 'topic-live' AND msg_id = 'message-deleted'",
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
            hash: Some("live-hash".to_string()),
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
            "SELECT content FROM messages WHERE topic_id = 'topic-live' AND msg_id = 'message-live'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(blob, vec![0x01, 0x02]);
    assert_eq!(
        tx.query_row(
            "SELECT content FROM messages_fts WHERE msg_id = 'message-live'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "live body"
    );
    assert_eq!(
        tx.query_row(
            "SELECT hash FROM message_attachments WHERE msg_id = 'message-live'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        "live-hash"
    );
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
        "INSERT INTO topics (topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at)
         VALUES ('topic-orphan', 'orphan', 'missing-agent', 'agent', 1, 1, 0, 1)",
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
