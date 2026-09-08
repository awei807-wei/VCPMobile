use super::messages::soft_delete_messages_data;
use super::storage::{soft_delete_owner_data, soft_delete_topic_data};
use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::ExpectedMessageStates;
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::sync_types::{
    MessageDeleteDecision, MessageLiveState, MessageVersionState,
};
use crate::vcp_modules::topic_types::{MessageKey, OwnerKey, TopicKey};
use sqlx::sqlite::SqlitePoolOptions;
use std::path::PathBuf;
use tauri::Manager;

fn topic(owner_type: &str, owner_id: &str, topic_id: &str) -> TopicKey {
    TopicKey::new(owner_type, owner_id, topic_id)
}

async fn test_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open delete fixture");
    sqlx::query(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, config_hash TEXT, content_hash TEXT,
            deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, config_hash TEXT, content_hash TEXT,
            deleted_at INTEGER
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
            timestamp INTEGER, updated_at INTEGER, content_hash TEXT,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE render_cache (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
         );
         CREATE TABLE message_attachments (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
         );
         CREATE TABLE active_generations (
            owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT
         );
         CREATE TABLE group_member_tags (
            group_id TEXT, agent_id TEXT, member_tag TEXT, updated_at INTEGER,
            PRIMARY KEY(group_id, agent_id)
         );
         INSERT INTO agents VALUES ('agent-a', '', 'agent-before', NULL);
         INSERT INTO groups VALUES ('group-a', '', 'group-before', NULL);",
    )
    .execute(&pool)
    .await
    .expect("create delete fixture");
    for (owner_type, owner_id) in [("agent", "agent-a"), ("group", "group-a")] {
        sqlx::query(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, locked, unread,
                msg_count, updated_at, last_message_updated_at, config_hash,
                content_hash, deleted_at
             ) VALUES (?, ?, 'shared', ?, 1, 0, 0, 1, 7, 0, ?, ?, NULL)",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(format!("{owner_type} topic"))
        .bind(format!("{owner_type}-config"))
        .bind(format!("{owner_type}-topic"))
        .execute(&pool)
        .await
        .expect("insert topic");
        sqlx::query(
            "INSERT INTO messages (
                owner_type, owner_id, topic_id, msg_id, timestamp, updated_at,
                content_hash, deleted_at
             ) VALUES (?, ?, 'shared', 'm1', 1, 9, ?, NULL)",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(format!("{owner_type}-message"))
        .execute(&pool)
        .await
        .expect("insert message");
        sqlx::query(
            "INSERT INTO render_cache VALUES (?, ?, 'shared', 'm1');
             INSERT INTO message_attachments VALUES (?, ?, 'shared', 'm1');
             INSERT INTO active_generations VALUES (?, ?, 'shared', 'm1');",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(owner_type)
        .bind(owner_id)
        .bind(owner_type)
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("insert side tables");
    }
    sqlx::query(
        "INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
         VALUES ('group-a', 'agent-a', '群组标签', 1),
                ('group-other', 'agent-a', '其他群组标签', 1)",
    )
    .execute(&pool)
    .await
    .expect("insert group member tags");
    pool
}

async fn count(pool: &sqlx::SqlitePool, table: &str, key: &TopicKey) -> i64 {
    let sql = format!(
        "SELECT COUNT(*) FROM {table}
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
    );
    sqlx::query_scalar(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(pool)
        .await
        .expect("count side table")
}

#[tokio::test]
async fn topic_delete_isolated_by_owner_namespace_and_clears_side_tables() {
    let pool = test_pool().await;
    let target = topic("agent", "agent-a", "shared");
    let unrelated = topic("group", "group-a", "shared");
    let receipt = soft_delete_topic_data(&pool, &target, 50)
        .await
        .expect("delete topic");

    assert_eq!(
        receipt.active_messages,
        vec![MessageKey::new(target.clone(), "m1")]
    );
    let target_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read target topic");
    assert_eq!(target_deleted, Some(50));
    let target_message: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read target message");
    assert_eq!(target_message, Some(50));
    assert_eq!(count(&pool, "render_cache", &target).await, 0);
    assert_eq!(count(&pool, "message_attachments", &target).await, 0);
    assert_eq!(count(&pool, "active_generations", &target).await, 0);
    assert_eq!(count(&pool, "render_cache", &unrelated).await, 1);
    let unrelated_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'group-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read unrelated message");
    assert_eq!(unrelated_deleted, None);
}

#[tokio::test]
async fn message_batch_is_monotonic_and_missing_topic_is_a_noop() {
    let pool = test_pool().await;
    let target = topic("agent", "agent-a", "shared");
    let receipt = soft_delete_messages_data(
        &pool,
        &target,
        &[MessageDeleteDecision {
            msg_id: "m1".to_string(),
            deleted_at: 30,
        }],
        true,
        None,
    )
    .await
    .expect("delete message");
    assert_eq!(
        receipt.active_messages,
        vec![MessageKey::new(target.clone(), "m1")]
    );
    let activity: (i64, i64, Option<i64>) = sqlx::query_as(
        "SELECT msg_count, last_message_updated_at, deleted_at FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read target activity");
    assert_eq!(activity, (0, 30, None));
    assert_eq!(count(&pool, "active_generations", &target).await, 0);

    let missing = topic("agent", "agent-a", "does-not-exist");
    soft_delete_messages_data(
        &pool,
        &missing,
        &[MessageDeleteDecision {
            msg_id: "m1".to_string(),
            deleted_at: 40,
        }],
        true,
        None,
    )
    .await
    .expect("missing topic no-op");
    let missing_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'does-not-exist'",
    )
    .fetch_one(&pool)
    .await
    .expect("read missing topic");
    assert_eq!(missing_rows, 0);
}

#[tokio::test]
async fn message_delete_rejects_a_local_edit_after_the_phase3_snapshot() {
    let pool = test_pool().await;
    let target = topic("agent", "agent-a", "shared");
    let expected = ExpectedMessageStates::from([(
        "m1".to_string(),
        Some(MessageVersionState::Live(MessageLiveState {
            message_hash: "agent-message".to_string(),
            updated_at: 9,
        })),
    )]);
    sqlx::query(
        "UPDATE messages SET content_hash = 'local-edit', updated_at = 10
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'shared' AND msg_id = 'm1'",
    )
    .execute(&pool)
    .await
    .expect("simulate local edit after snapshot");
    let error = soft_delete_messages_data(
        &pool,
        &target,
        &[MessageDeleteDecision {
            msg_id: "m1".to_string(),
            deleted_at: 30,
        }],
        false,
        Some(&expected),
    )
    .await
    .expect_err("stale remote tombstone must not delete the local edit");
    assert!(error.contains("SYNC_SNAPSHOT_STALE"));
    let state: (String, i64, Option<i64>) = sqlx::query_as(
        "SELECT content_hash, updated_at, deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'shared' AND msg_id = 'm1'",
    )
    .fetch_one(&pool)
    .await
    .expect("local edit should remain live");
    assert_eq!(state, ("local-edit".to_string(), 10, None));
    assert_eq!(count(&pool, "active_generations", &target).await, 1);
}

#[tokio::test]
async fn message_delete_rolls_back_tombstone_and_hash_cleanup_on_hash_failure() {
    let pool = test_pool().await;
    sqlx::query(
        "CREATE TRIGGER fail_agent_hash
         BEFORE UPDATE OF content_hash ON agents
         BEGIN SELECT RAISE(ABORT, 'hash failure'); END;",
    )
    .execute(&pool)
    .await
    .expect("install hash failure trigger");
    let target = topic("agent", "agent-a", "shared");
    let error = soft_delete_messages_data(
        &pool,
        &target,
        &[MessageDeleteDecision {
            msg_id: "m1".to_string(),
            deleted_at: 30,
        }],
        true,
        None,
    )
    .await
    .expect_err("hash failure must roll back delete");
    assert!(error.contains("hash failure"));
    let deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read rolled back message");
    assert_eq!(deleted, None);
    assert_eq!(count(&pool, "active_generations", &target).await, 1);
}

#[tokio::test]
async fn owner_delete_cascades_only_its_namespace() {
    let pool = test_pool().await;
    let owner = OwnerKey::new("agent", "agent-a");
    let unrelated = topic("group", "group-a", "shared");
    let receipt = soft_delete_owner_data(&pool, &owner, 60)
        .await
        .expect("delete owner");
    assert_eq!(
        receipt.active_messages,
        vec![MessageKey::new(topic("agent", "agent-a", "shared"), "m1",)]
    );
    let owner_deleted: Option<i64> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'")
            .fetch_one(&pool)
            .await
            .expect("read owner");
    assert_eq!(owner_deleted, Some(60));
    assert_eq!(
        count(
            &pool,
            "active_generations",
            &topic("agent", "agent-a", "shared")
        )
        .await,
        0
    );
    assert_eq!(count(&pool, "active_generations", &unrelated).await, 1);
    let unrelated_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'group-a' AND topic_id = 'shared'",
    )
    .fetch_one(&pool)
    .await
    .expect("read unrelated message");
    assert_eq!(unrelated_deleted, None);
}

#[tokio::test]
async fn group_owner_delete_clears_only_its_persistent_member_tags() {
    let pool = test_pool().await;
    sqlx::query("INSERT INTO groups VALUES ('group-other', '', 'other-before', NULL)")
        .execute(&pool)
        .await
        .expect("insert unrelated group");

    soft_delete_owner_data(&pool, &OwnerKey::new("group", "group-a"), 70)
        .await
        .expect("delete group owner");
    let deleted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_member_tags WHERE group_id = 'group-a'")
            .fetch_one(&pool)
            .await
            .expect("count deleted group tags");
    assert_eq!(deleted, 0);
    let retained: String = sqlx::query_scalar(
        "SELECT member_tag FROM group_member_tags
         WHERE group_id = 'group-other' AND agent_id = 'agent-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("read unrelated group tag");
    assert_eq!(retained, "其他群组标签");
}

fn command_app(pool: sqlx::SqlitePool) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool,
        path: PathBuf::from("test.sqlite"),
    });
    app.manage(AgentConfigState::new());
    app.manage(GroupManagerState::new());
    app
}

#[tokio::test]
async fn owner_delete_wrapper_uses_owner_lock_for_agent() {
    let app = command_app(test_pool().await);
    super::DeleteExecutor::soft_delete_agent(&app.handle(), "agent-a", 80)
        .await
        .expect("同步 Agent 删除包装应成功");
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        Some(80)
    );
}

#[tokio::test]
async fn owner_delete_wrapper_uses_owner_lock_for_group() {
    let app = command_app(test_pool().await);
    super::DeleteExecutor::soft_delete_group(&app.handle(), "group-a", 90)
        .await
        .expect("同步 Group 删除包装应成功");
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM groups WHERE group_id = 'group-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        Some(90)
    );
}
