use super::*;

async fn test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open delete contract database");
    sqlx::query(
        "CREATE TABLE agents (
           agent_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL, deleted_at INTEGER
         );
         CREATE TABLE groups (
           group_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL, deleted_at INTEGER
         );
         CREATE TABLE topics (
           topic_id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, owner_type TEXT NOT NULL,
           title TEXT NOT NULL, created_at INTEGER NOT NULL, locked INTEGER NOT NULL,
           unread INTEGER NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL,
           deleted_at INTEGER
         );
         CREATE TABLE messages (
           topic_id TEXT NOT NULL, msg_id TEXT NOT NULL, timestamp INTEGER NOT NULL,
           content_hash TEXT NOT NULL, deleted_at INTEGER,
           PRIMARY KEY(topic_id, msg_id)
         );
         CREATE TABLE active_generations (
           owner_id TEXT, owner_type TEXT, topic_id TEXT, msg_id TEXT
         );
         CREATE TABLE render_cache (topic_id TEXT, msg_id TEXT);
         CREATE TABLE message_attachments (topic_id TEXT, msg_id TEXT);",
    )
    .execute(&pool)
    .await
    .expect("create delete contract schema");
    pool
}

#[tokio::test]
async fn owner_tombstone_keeps_remote_timestamp_and_cascades_atomically() {
    let pool = test_pool().await;
    sqlx::query(
        "INSERT INTO agents VALUES ('agent-a', 'owner-old', NULL);
         INSERT INTO topics VALUES
           ('topic-a', 'agent-a', 'agent', 'A', 1, 1, 0, 'cfg', 'topic-old', NULL);
         INSERT INTO messages VALUES ('topic-a', 'msg-a', 1, 'msg-old', NULL);
         INSERT INTO active_generations VALUES ('agent-a', 'agent', 'topic-a', 'msg-a');",
    )
    .execute(&pool)
    .await
    .expect("seed owner tombstone fixture");

    let active_ids = soft_delete_owner_data(
        &pool,
        OwnerDeleteSpec {
            table: "agents",
            id_column: "agent_id",
            owner_type: "agent",
        },
        "agent-a",
        4242,
    )
    .await
    .expect("apply owner tombstone");

    assert_eq!(active_ids, vec!["msg-a"]);
    for (table, column, id) in [
        ("agents", "agent_id", "agent-a"),
        ("topics", "topic_id", "topic-a"),
        ("messages", "msg_id", "msg-a"),
    ] {
        let sql = format!("SELECT deleted_at FROM {table} WHERE {column} = ?");
        let deleted_at: i64 = sqlx::query_scalar(&sql)
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read tombstone");
        assert_eq!(deleted_at, 4242);
    }
    let active_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_generations")
        .fetch_one(&pool)
        .await
        .expect("count active generations");
    assert_eq!(active_count, 0);
}

#[tokio::test]
async fn message_tombstone_clears_side_tables_and_recomputes_hashes() {
    let pool = test_pool().await;
    sqlx::query(
        "INSERT INTO agents VALUES ('agent-a', 'owner-old', NULL);
         INSERT INTO topics VALUES
           ('topic-a', 'agent-a', 'agent', 'A', 1, 1, 0, 'cfg-old', 'topic-old', NULL);
         INSERT INTO messages VALUES
           ('topic-a', 'msg-a', 1, 'msg-old-a', NULL),
           ('topic-a', 'msg-b', 2, 'msg-live-b', NULL);
         INSERT INTO active_generations VALUES ('agent-a', 'agent', 'topic-a', 'msg-a');
         INSERT INTO render_cache VALUES ('topic-a', 'msg-a');
         INSERT INTO message_attachments VALUES ('topic-a', 'msg-a');",
    )
    .execute(&pool)
    .await
    .expect("seed message tombstone fixture");

    assert!(soft_delete_message_data(&pool, "topic-a", "msg-a", 5151)
        .await
        .expect("apply message tombstone"));
    let deleted_at: i64 = sqlx::query_scalar(
        "SELECT deleted_at FROM messages WHERE topic_id = 'topic-a' AND msg_id = 'msg-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("read message tombstone");
    assert_eq!(deleted_at, 5151);
    for table in ["active_generations", "render_cache", "message_attachments"] {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let count: i64 = sqlx::query_scalar(&sql)
            .fetch_one(&pool)
            .await
            .expect("count cleared side table");
        assert_eq!(count, 0, "{table} must be cleared");
    }
    let (topic_hash, owner_hash): (String, String) = sqlx::query_as(
        "SELECT topics.content_hash, agents.content_hash
         FROM topics JOIN agents ON agents.agent_id = topics.owner_id
         WHERE topics.topic_id = 'topic-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("read recomputed hashes");
    assert_ne!(topic_hash, "topic-old");
    assert_ne!(owner_hash, "owner-old");
}
