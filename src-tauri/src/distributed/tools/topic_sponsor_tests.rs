use super::*;
use crate::vcp_modules::message_repository::ContentCompressor;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;

#[test]
fn topic_sponsor_manifest_exposes_create_topic() {
    let manifest = TopicSponsorTool.manifest();
    assert_eq!(manifest.name, "MobileTopicSponsor");
    assert_eq!(
        manifest.invocation_commands[0].command_identifier,
        "MobileTopicSponsor"
    );
    assert!(manifest
        .invocation_commands
        .iter()
        .any(|command| command.description.contains("CreateTopic")));
}

#[test]
fn parses_legacy_topic_id_aliases() {
    assert_eq!(
        get_topic_id_arg(&json!({ "TopicId": "topic_1" })).as_deref(),
        Some("topic_1")
    );
}

#[test]
fn escapes_sql_like_wildcards() {
    assert_eq!(escape_like_pattern(r"agent_%\name"), r"agent\_\%\\name");
}

#[tokio::test]
async fn agent_lookup_treats_like_wildcards_literally() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            updated_at BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agents (agent_id, name, updated_at, deleted_at)
         VALUES
            ('agent_1', 'Alpha', 1, NULL),
            ('agent_2', 'Beta', 2, NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let wildcard = find_agent_info(&pool, "%").await;
    assert!(wildcard.is_err());

    sqlx::query(
        "INSERT INTO agents (agent_id, name, updated_at, deleted_at)
         VALUES ('agent_3', 'agent_%', 3, NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let exact = find_agent_info(&pool, "agent_%").await.unwrap();
    assert_eq!(exact.id, "agent_3");
}

#[tokio::test]
async fn load_messages_keeps_valid_messages_when_one_is_corrupt() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            role TEXT NOT NULL,
            name TEXT,
            content BLOB NOT NULL,
            timestamp BIGINT NOT NULL,
            agent_id TEXT,
            finish_reason TEXT,
            deleted_at BIGINT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
         VALUES ('agent', 'agent_1', 'topic_1', 'msg_1', 'assistant', 'Alpha', ?, 1, 'agent_1', 'completed', NULL)",
    )
    .bind(ContentCompressor::compress("valid content").unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
         VALUES ('agent', 'agent_1', 'topic_1', 'msg_2', 'assistant', 'Alpha', ?, 2, 'agent_1', 'completed', NULL)",
    )
    .bind(vec![1_u8, 2, 3])
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, role, name, content, timestamp, agent_id, finish_reason, deleted_at)
         VALUES ('group', 'group_1', 'topic_1', 'msg_group', 'assistant', 'Group', ?, 3, NULL, 'completed', NULL)",
    )
    .bind(ContentCompressor::compress("must stay isolated").unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let messages = load_messages(&pool, "agent_1", "topic_1").await.unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["content"], "valid content");
    assert_eq!(messages[0]["contentCorrupted"], false);
    assert_eq!(messages[1]["contentCorrupted"], true);
    assert!(messages[1]["content"]
        .as_str()
        .unwrap()
        .contains("解压失败"));
}

#[tokio::test]
async fn first_message_name_returns_none_for_null_name() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            name TEXT,
            timestamp BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, name, timestamp, deleted_at)
         VALUES ('agent', 'agent_1', 'topic_1', 'msg_1', NULL, 1, NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let name = first_message_name(&pool, "agent_1", "topic_1")
        .await
        .unwrap();

    assert_eq!(name, None);
}

#[tokio::test]
async fn first_message_name_returns_first_non_null_name() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            name TEXT,
            timestamp BIGINT NOT NULL,
            deleted_at BIGINT
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, name, timestamp, deleted_at)
         VALUES
            ('agent', 'agent_1', 'topic_1', 'msg_1', 'creator', 1, NULL),
            ('agent', 'agent_1', 'topic_1', 'msg_2', NULL, 2, NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let name = first_message_name(&pool, "agent_1", "topic_1")
        .await
        .unwrap();

    assert_eq!(name.as_deref(), Some("creator"));
}

async fn sponsor_write_test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT '', deleted_at INTEGER
         );
         CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0, msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT '', content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content BLOB NOT NULL, timestamp INTEGER NOT NULL,
            is_group_message INTEGER NOT NULL DEFAULT 0, group_id TEXT,
            finish_reason TEXT, content_hash TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE render_cache (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, render_content BLOB, updated_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT NOT NULL DEFAULT '', renderer_schema_version INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id, attachment_order)
         );
         CREATE TABLE active_generations (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE messages_fts (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, content TEXT NOT NULL,
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL
         );
         CREATE TABLE message_unread_receipts (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at BIGINT NOT NULL,
            counted_unread INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         INSERT INTO agents(agent_id) VALUES ('agent-1');",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn sponsor_create_and_reply_write_idempotent_unread_receipts() {
    let pool = sponsor_write_test_pool().await;
    let key = TopicKey::new("agent", "agent-1", "topic-1");
    let agent = AgentInfo {
        id: "agent-1".to_string(),
        name: "Agent".to_string(),
    };
    let initial = build_chat_message(
        "create-message".to_string(),
        &agent,
        key.topic_id.clone(),
        "initial".to_string(),
        1,
    );
    persist_new_topic(&pool, &key, &initial, "Topic", 1)
        .await
        .unwrap();

    let topic = TopicRow {
        id: key.topic_id.clone(),
        name: "Topic".to_string(),
        created_at: 1,
        locked: false,
        unread: true,
        unread_count: 1,
        msg_count: 1,
        owner_id: key.owner_id.clone(),
        owner_type: key.owner_type.clone(),
    };
    let reply = build_chat_message(
        "reply-message".to_string(),
        &agent,
        key.topic_id.clone(),
        "reply".to_string(),
        2,
    );
    persist_topic_reply(&pool, &reply, &topic, 2).await.unwrap();

    let state: (i32, i32, i32) = sqlx::query_as(
        "SELECT unread, unread_count, msg_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, (1, 2, 2));
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 2);

    crate::vcp_modules::chat::message_service::delete_messages_for_topic(
        &pool,
        &key,
        vec!["reply-message".to_string()],
    )
    .await
    .unwrap();
    let remaining: i32 = sqlx::query_scalar(
        "SELECT unread_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1);
}
