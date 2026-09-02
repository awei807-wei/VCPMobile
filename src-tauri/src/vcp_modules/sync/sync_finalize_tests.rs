use super::finalize_modified_topics;
use std::collections::HashSet;

#[tokio::test]
async fn late_owner_hash_failure_rolls_back_all_finalizer_updates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open test database");
    sqlx::query(
        "CREATE TABLE agents (
                agent_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE groups (
                group_id TEXT PRIMARY KEY, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, title TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
                updated_at INTEGER, config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, timestamp INTEGER,
                content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO agents VALUES ('agent', 'owner-before', NULL);
             INSERT INTO topics VALUES
                ('topic', 'agent', 'agent', 'Topic', 1, 1, 0, 0, 1,
                 'config-before', 'content-before', NULL);
             INSERT INTO messages VALUES ('topic', 'message', 1, 'message-hash', NULL);
             CREATE TRIGGER fail_owner_hash
             BEFORE UPDATE OF content_hash ON agents
             BEGIN SELECT RAISE(ABORT, 'owner hash failure'); END;",
    )
    .execute(&pool)
    .await
    .expect("create finalizer fixture");

    let error = finalize_modified_topics(&pool, &HashSet::from(["topic".to_string()]))
        .await
        .expect_err("owner hash failure must fail finalization");
    assert!(error.contains("owner hash failure"));
    let row: (i64, String, String) = sqlx::query_as(
        "SELECT msg_count, config_hash, content_hash FROM topics WHERE topic_id = 'topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("read rolled-back topic");
    assert_eq!(row.0, 0);
    assert_eq!(row.1, "config-before");
    assert_eq!(row.2, "content-before");
}

#[tokio::test]
async fn metadata_query_failure_is_not_reported_as_success() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open test database");
    sqlx::query("CREATE TABLE topics (topic_id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .expect("create malformed topics table");

    let error = finalize_modified_topics(&pool, &HashSet::from(["topic".to_string()]))
        .await
        .expect_err("malformed metadata query must fail closed");
    assert!(error.contains("话题元数据"));
}

#[tokio::test]
async fn missing_or_tombstoned_repair_topic_fails_before_updates() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open test database");
    sqlx::query(
        "CREATE TABLE topics (
                topic_id TEXT PRIMARY KEY, owner_id TEXT, owner_type TEXT, title TEXT,
                created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
                updated_at INTEGER, config_hash TEXT, content_hash TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                topic_id TEXT, msg_id TEXT, timestamp INTEGER,
                content_hash TEXT, deleted_at INTEGER
             );
             INSERT INTO topics VALUES
                ('live', 'agent', 'agent', 'Live', 1, 1, 0, 7, 1, '', '', NULL),
                ('deleted', 'agent', 'agent', 'Deleted', 1, 1, 0, 9, 1, '', '', 8);",
    )
    .execute(&pool)
    .await
    .expect("create finalizer fixture");

    for missing in ["missing", "deleted"] {
        let error = finalize_modified_topics(
            &pool,
            &HashSet::from(["live".to_string(), missing.to_string()]),
        )
        .await
        .expect_err("repair set must have exact live metadata coverage");
        assert!(error.contains(missing));
        let msg_count: i64 =
            sqlx::query_scalar("SELECT msg_count FROM topics WHERE topic_id = 'live'")
                .fetch_one(&pool)
                .await
                .expect("read unchanged topic");
        assert_eq!(msg_count, 7);
    }
}
