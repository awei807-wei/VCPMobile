use super::finalize_modified_topics;
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::HashSet;

fn topic(owner_type: &str, owner_id: &str, topic_id: &str) -> TopicKey {
    TopicKey::new(owner_type, owner_id, topic_id)
}

async fn test_pool() -> sqlx::SqlitePool {
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
            owner_type TEXT, owner_id TEXT, topic_id TEXT, title TEXT,
            created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
            updated_at INTEGER, last_message_updated_at INTEGER,
            config_hash TEXT, content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
            timestamp INTEGER, updated_at INTEGER, content_hash TEXT, deleted_at INTEGER
         );
         INSERT INTO agents VALUES ('agent-a', 'agent-before', NULL);
         INSERT INTO groups VALUES ('group-a', 'group-before', NULL);
         INSERT INTO topics VALUES
            ('agent', 'agent-a', 'shared', 'Agent topic', 1, 1, 0, 99, 77, 0,
             'agent-config-before', 'agent-topic-before', NULL),
            ('group', 'group-a', 'shared', 'Group topic', 1, 1, 0, 88, 66, 0,
             'group-config-before', 'group-topic-before', NULL);
         INSERT INTO messages VALUES
            ('agent', 'agent-a', 'shared', 'agent-live', 1, 9, 'agent-message', NULL),
            ('agent', 'agent-a', 'shared', 'agent-deleted', 2, 10, 'agent-deleted-message', 42),
            ('group', 'group-a', 'shared', 'group-live', 1, 19, 'group-message', NULL);",
    )
    .execute(&pool)
    .await
    .expect("create finalizer fixture");
    pool
}

#[tokio::test]
async fn finalizer_refreshes_activity_without_advancing_config_time() {
    let pool = test_pool().await;
    finalize_modified_topics(&pool, &HashSet::from([topic("agent", "agent-a", "shared")]))
        .await
        .expect("finalize topic");

    let state: (i64, i64, i64, String, String) = sqlx::query_as(
        "SELECT updated_at, last_message_updated_at, msg_count,
                config_hash, content_hash
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read finalized state");
    assert_eq!(state.0, 77);
    assert_eq!(state.1, 42);
    assert_eq!(state.2, 1);
    assert_ne!(state.3, "agent-config-before");
    assert_ne!(state.4, "agent-topic-before");
    let owner_hash: String =
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'agent-a'")
            .fetch_one(&pool)
            .await
            .expect("read owner hash");
    assert_ne!(owner_hash, "agent-before");
}

#[tokio::test]
async fn same_topic_id_isolated_across_owner_namespaces() {
    let pool = test_pool().await;
    finalize_modified_topics(
        &pool,
        &HashSet::from([
            topic("agent", "agent-a", "shared"),
            topic("group", "group-a", "shared"),
        ]),
    )
    .await
    .expect("finalize both namespaces");

    let agent_count: i64 = sqlx::query_scalar(
        "SELECT msg_count FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read agent count");
    let group_count: i64 = sqlx::query_scalar(
        "SELECT msg_count FROM topics
         WHERE owner_type = 'group' AND owner_id = 'group-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read group count");
    assert_eq!(agent_count, 1);
    assert_eq!(group_count, 1);

    let owner_hashes: (String, String) = sqlx::query_as(
        "SELECT a.content_hash, g.content_hash
         FROM agents a CROSS JOIN groups g
         WHERE a.agent_id = 'agent-a' AND g.group_id = 'group-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("read owner hashes");
    assert_ne!(owner_hashes.0, "agent-before");
    assert_ne!(owner_hashes.1, "group-before");
}

#[tokio::test]
async fn late_owner_hash_failure_rolls_back_all_finalizer_updates() {
    let pool = test_pool().await;
    sqlx::query(
        "CREATE TRIGGER fail_owner_hash
         BEFORE UPDATE OF content_hash ON agents
         BEGIN SELECT RAISE(ABORT, 'owner hash failure'); END;",
    )
    .execute(&pool)
    .await
    .expect("install failure trigger");

    let error =
        finalize_modified_topics(&pool, &HashSet::from([topic("agent", "agent-a", "shared")]))
            .await
            .expect_err("owner hash failure must fail finalization");
    assert!(error.contains("owner hash failure"));
    let row: (i64, i64, String, String) = sqlx::query_as(
        "SELECT msg_count, last_message_updated_at, config_hash, content_hash
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read rolled-back topic");
    assert_eq!(row.0, 99);
    assert_eq!(row.1, 0);
    assert_eq!(row.2, "agent-config-before");
    assert_eq!(row.3, "agent-topic-before");
}

#[tokio::test]
async fn missing_or_tombstoned_repair_topic_fails_before_updates() {
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
            owner_type TEXT, owner_id TEXT, topic_id TEXT, title TEXT,
            created_at INTEGER, locked INTEGER, unread INTEGER, msg_count INTEGER,
            updated_at INTEGER, last_message_updated_at INTEGER,
            config_hash TEXT, content_hash TEXT, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
            timestamp INTEGER, updated_at INTEGER, content_hash TEXT, deleted_at INTEGER
         );
         INSERT INTO agents VALUES ('agent-a', 'before', NULL);
         INSERT INTO topics VALUES
            ('agent', 'agent-a', 'live', 'Live', 1, 1, 0, 7, 1, 0, '', '', NULL),
            ('agent', 'agent-a', 'deleted', 'Deleted', 1, 1, 0, 9, 1, 0, '', '', 8);",
    )
    .execute(&pool)
    .await
    .expect("create finalizer fixture");

    for missing in ["missing", "deleted"] {
        let error = finalize_modified_topics(
            &pool,
            &HashSet::from([
                topic("agent", "agent-a", "live"),
                topic("agent", "agent-a", missing),
            ]),
        )
        .await
        .expect_err("repair set must have exact live metadata coverage");
        assert!(error.contains(missing));
        let msg_count: i64 = sqlx::query_scalar(
            "SELECT msg_count FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'live'",
        )
        .fetch_one(&pool)
        .await
        .expect("read unchanged topic");
        assert_eq!(msg_count, 7);
    }
}
