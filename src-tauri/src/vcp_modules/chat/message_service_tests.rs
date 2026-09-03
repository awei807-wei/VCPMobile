use super::{
    delete_messages_for_topic, load_multi_topic_messages, load_multi_topic_messages_for_keys,
    truncate_history_after_timestamp_for_topic,
};
use crate::vcp_modules::topic_types::TopicKey;

async fn test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open test database");
    sqlx::raw_sql(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT 'before',
            deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT 'before',
            deleted_at INTEGER
         );
         CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT 'before',
            content_hash TEXT NOT NULL DEFAULT 'before', deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content BLOB NOT NULL, timestamp INTEGER NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT 0, is_group_message INTEGER NOT NULL DEFAULT 0,
            group_id TEXT, finish_reason TEXT, content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
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
            display_name TEXT NOT NULL, src TEXT, status TEXT, created_at INTEGER NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id, attachment_order)
         );
         CREATE TABLE active_generations (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
            internal_path TEXT NOT NULL, extracted_text TEXT, image_frames TEXT,
            thumbnail_path TEXT, created_at INTEGER NOT NULL
         );
         CREATE TABLE messages_fts (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, content TEXT NOT NULL,
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL
         );",
    )
    .execute(&pool)
    .await
    .expect("create message service schema");
    pool
}

async fn insert_message(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    content: &str,
    with_attachment: bool,
) {
    let compressed = zstd::bulk::compress(content.as_bytes(), 3).expect("compress message");
    let (table, column) = match key.owner_type.as_str() {
        "agent" => ("agents", "agent_id"),
        "group" => ("groups", "group_id"),
        other => panic!("unsupported test owner type {other}"),
    };
    sqlx::query(&format!(
        "INSERT OR IGNORE INTO {table}({column}) VALUES (?)"
    ))
    .bind(&key.owner_id)
    .execute(pool)
    .await
    .expect("insert owner");
    sqlx::query("INSERT INTO topics(owner_type, owner_id, topic_id) VALUES (?, ?, ?)")
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(pool)
        .await
        .expect("insert topic");
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, role, content, timestamp, updated_at)
         VALUES (?, ?, ?, 'same-message', 'user', ?, 100, 101)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(compressed)
    .execute(pool)
    .await
    .expect("insert message");
    sqlx::query(
        "INSERT INTO render_cache(owner_type, owner_id, topic_id, msg_id, render_content)
         VALUES (?, ?, ?, 'same-message', ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(Vec::<u8>::new())
    .execute(pool)
    .await
    .expect("insert cache");
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES ('same-message', ?, ?, ?, ?)",
    )
    .bind(&key.topic_id)
    .bind(content)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(pool)
    .await
    .expect("insert fts");
    if with_attachment {
        sqlx::query(
            "INSERT INTO attachments(hash, mime_type, size, internal_path, created_at)
             VALUES ('hash-a', 'text/plain', 1, '', 100)",
        )
        .execute(pool)
        .await
        .expect("insert attachment");
        sqlx::query(
            "INSERT INTO message_attachments(
                owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                display_name, created_at
             ) VALUES (?, ?, ?, 'same-message', 'hash-a', 7, 'ordered.txt', 100)",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(pool)
        .await
        .expect("insert message attachment");
    }
}

#[tokio::test]
async fn batch_loader_keeps_same_topic_and_message_id_isolated() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", true).await;
    insert_message(&pool, &group, "group body", false).await;
    let loaded = load_multi_topic_messages_for_keys(&pool, &[agent.clone(), group.clone()])
        .await
        .expect("load owner-scoped messages");
    assert_eq!(loaded[&agent][0].content, "agent body");
    assert_eq!(loaded[&agent][0].updated_at, Some(101));
    assert_eq!(
        loaded[&agent][0].attachments.as_ref().unwrap()[0].attachment_order,
        Some(7)
    );
    assert_eq!(loaded[&group][0].content, "group body");
    assert!(loaded[&group][0].attachments.is_none());
}

#[tokio::test]
async fn owner_scoped_delete_does_not_touch_other_owner() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", true).await;
    insert_message(&pool, &group, "group body", false).await;
    sqlx::query(
        "UPDATE messages SET deleted_at = 9000000000000
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .execute(&pool)
    .await
    .expect("seed future tombstone");
    delete_messages_for_topic(&pool, &agent, vec!["same-message".to_string()])
        .await
        .expect("delete agent message");
    let agent_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load agent tombstone");
    assert_eq!(agent_deleted, Some(9000000000000));
    let group_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'owner-g' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load group message");
    assert!(group_deleted.is_none());
    let group_fts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages_fts
         WHERE owner_type = 'group' AND owner_id = 'owner-g'",
    )
    .fetch_one(&pool)
    .await
    .expect("count group fts");
    assert_eq!(group_fts, 1);
    let agent_attachments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("count deleted attachments");
    assert_eq!(agent_attachments, 0);
    let (topic_hash, owner_hash): (String, String) = sqlx::query_as(
        "SELECT t.content_hash, a.content_hash
         FROM topics t JOIN agents a ON a.agent_id = t.owner_id
         WHERE t.owner_type = 'agent' AND t.owner_id = 'owner-a'
           AND t.topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("read bubbled hashes");
    assert!(topic_hash.is_empty());
    assert!(owner_hash.len() == 64 && owner_hash != "before");
}

#[tokio::test]
async fn legacy_batch_loader_rejects_ambiguous_topic_id() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", false).await;
    insert_message(&pool, &group, "group body", false).await;
    let error = load_multi_topic_messages(&pool, &["shared-topic".to_string()])
        .await
        .expect_err("legacy loader must fail closed");
    assert!(error.contains("ambiguous"));
}

#[tokio::test]
async fn owner_scoped_truncate_keeps_other_owner_history() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", false).await;
    insert_message(&pool, &group, "group body", false).await;
    sqlx::query(
        "UPDATE messages SET timestamp = 200
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .execute(&pool)
    .await
    .expect("raise agent message timestamp");
    truncate_history_after_timestamp_for_topic(&pool, &agent, 150)
        .await
        .expect("truncate agent history");
    let agent_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load agent tombstone");
    assert!(agent_deleted.is_some());
    let group_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'owner-g' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load group message");
    assert!(group_deleted.is_none());
    let group_fts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages_fts
         WHERE owner_type = 'group' AND owner_id = 'owner-g'",
    )
    .fetch_one(&pool)
    .await
    .expect("count group fts");
    assert_eq!(group_fts, 1);
}
