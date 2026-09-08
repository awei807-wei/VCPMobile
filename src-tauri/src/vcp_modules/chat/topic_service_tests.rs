use super::{increment_topic_unread_count_in_pool, set_topic_unread_in_pool, TopicKey};
use futures_util::future::join_all;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::PathBuf;

pub(super) async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    create_schema(&pool).await;
    seed_owner(&pool, "agent", "owner-a").await;
    seed_owner(&pool, "agent", "shared-owner").await;
    seed_owner(&pool, "group", "shared-owner").await;
    pool
}

async fn concurrent_test_pool() -> (SqlitePool, PathBuf) {
    let suffix = crate::vcp_modules::infra::utils::now_millis();
    let path = std::env::temp_dir().join(format!(
        "vcp-mobile-topic-unread-{}-{suffix}.db",
        std::process::id()
    ));
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .unwrap();
    create_schema(&pool).await;
    seed_owner(&pool, "agent", "owner-a").await;
    (pool, path)
}

pub(super) async fn create_schema(pool: &SqlitePool) {
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE groups (
            group_id TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 1,
            unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            hash TEXT NOT NULL,
            attachment_order INTEGER NOT NULL,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id, attachment_order)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE active_generations (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE topic_commit_failure_parent (
            id INTEGER PRIMARY KEY
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE topic_commit_failure_child (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER NOT NULL,
            FOREIGN KEY (parent_id) REFERENCES topic_commit_failure_parent(id)
                DEFERRABLE INITIALLY DEFERRED
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE message_unread_receipts (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            counted_unread INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
}

pub(super) async fn seed_owner(pool: &SqlitePool, owner_type: &str, owner_id: &str) {
    let table = if owner_type == "agent" {
        "agents"
    } else {
        "groups"
    };
    let column = if owner_type == "agent" {
        "agent_id"
    } else {
        "group_id"
    };
    sqlx::query(&format!(
        "INSERT INTO {table} ({column}, content_hash) VALUES (?, '')"
    ))
    .bind(owner_id)
    .execute(pool)
    .await
    .unwrap();
}

pub(super) async fn seed_topic(
    pool: &SqlitePool,
    key: &TopicKey,
    unread: bool,
    unread_count: i32,
    deleted_at: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            unread, unread_count, config_hash, content_hash, deleted_at
        ) VALUES (?, ?, ?, '测试话题', 1, 1, ?, ?, 'before-config', 'before-content', ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(i32::from(unread))
    .bind(unread_count)
    .bind(deleted_at)
    .execute(pool)
    .await
    .unwrap();
}

pub(super) async fn topic_row(
    pool: &SqlitePool,
    key: &TopicKey,
) -> (i32, i32, i64, String, String) {
    sqlx::query_as(
        "SELECT unread, unread_count, updated_at, config_hash, content_hash
         FROM topics WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub(super) async fn seed_message(pool: &SqlitePool, key: &TopicKey, message_id: &str) {
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content_hash)
         VALUES (?, ?, ?, ?, 'message-before')",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO message_attachments(
            owner_type, owner_id, topic_id, msg_id, hash, attachment_order
         ) VALUES (?, ?, ?, ?, 'attachment-before', 1)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO active_generations(
            owner_type, owner_id, topic_id, msg_id, created_at
         ) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn 标记话题已读清除未读计数并返回权威状态() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "topic-1");
    seed_topic(&pool, &key, true, 3, None).await;
    let before = topic_row(&pool, &key).await;

    let state = set_topic_unread_in_pool(&pool, &key, false, 2)
        .await
        .unwrap();
    let after = topic_row(&pool, &key).await;

    assert_eq!(state.owner_type, "agent");
    assert_eq!(state.owner_id, "owner-a");
    assert_eq!(state.topic_id, "topic-1");
    assert!(!state.unread);
    assert_eq!(state.unread_count, 0);
    assert_eq!((after.0, after.1, after.2), (0, 0, 2));
    assert_ne!(before.3, after.3);
    assert_eq!(state.unread_count, after.1);
}

#[tokio::test]
async fn 标记话题未读保留已有计数() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "topic-1");
    seed_topic(&pool, &key, false, 5, None).await;

    let state = set_topic_unread_in_pool(&pool, &key, true, 2)
        .await
        .unwrap();
    let after = topic_row(&pool, &key).await;

    assert!(state.unread);
    assert_eq!(state.unread_count, 5);
    assert_eq!((after.0, after.1, after.2), (1, 5, 2));
}

#[tokio::test]
async fn 相同话题标识按复合所有者隔离() {
    let pool = test_pool().await;
    let agent_key = TopicKey::new("agent", "shared-owner", "shared-topic");
    let group_key = TopicKey::new("group", "shared-owner", "shared-topic");
    seed_topic(&pool, &agent_key, true, 3, None).await;
    seed_topic(&pool, &group_key, true, 7, None).await;

    let state = set_topic_unread_in_pool(&pool, &agent_key, false, 2)
        .await
        .unwrap();
    let agent = topic_row(&pool, &agent_key).await;
    let group = topic_row(&pool, &group_key).await;

    assert_eq!(state.owner_id, "shared-owner");
    assert_eq!((agent.0, agent.1, agent.2), (0, 0, 2));
    assert_eq!((group.0, group.1, group.2), (1, 7, 1));
}

#[tokio::test]
async fn 并发未读计数递增保持原子并返回权威状态() {
    let (pool, path) = concurrent_test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "topic-1");
    seed_topic(&pool, &key, false, 0, None).await;
    for index in 0..24 {
        seed_message(&pool, &key, &format!("message-{index}")).await;
    }

    let tasks = (0..24).map(|index| {
        let pool = pool.clone();
        let key = key.clone();
        tokio::spawn(async move {
            increment_topic_unread_count_in_pool(&pool, &key, &format!("message-{index}"), true, 3)
                .await
                .unwrap()
        })
    });
    let states = join_all(tasks).await;
    let row = topic_row(&pool, &key).await;
    pool.close().await;
    let _ = std::fs::remove_file(path);

    assert_eq!(states.len(), 24);
    assert!(states.iter().all(|state| state.is_ok()));
    assert_eq!((row.0, row.1), (1, 24));
    let returned_counts: Vec<i32> = states
        .into_iter()
        .map(|state| state.unwrap().unread_count)
        .collect();
    assert!(returned_counts.contains(&24));
}

#[tokio::test]
async fn 缺失或已删除话题拒绝修改() {
    let pool = test_pool().await;
    let missing = TopicKey::new("agent", "owner-a", "missing");
    assert!(
        increment_topic_unread_count_in_pool(&pool, &missing, "missing-msg", true, 2)
            .await
            .is_err()
    );

    let deleted = TopicKey::new("agent", "owner-a", "deleted");
    seed_topic(&pool, &deleted, true, 4, Some(9)).await;
    assert!(set_topic_unread_in_pool(&pool, &deleted, false, 2)
        .await
        .is_err());
    let row = topic_row(&pool, &deleted).await;
    assert_eq!((row.0, row.1, row.2), (1, 4, 1));
}

#[tokio::test]
async fn 未读状态变化同步刷新哈希() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "topic-1");
    seed_topic(&pool, &key, false, 0, None).await;
    seed_message(&pool, &key, "message-1").await;
    let before = topic_row(&pool, &key).await;

    let state = increment_topic_unread_count_in_pool(&pool, &key, "message-1", true, 2)
        .await
        .unwrap();
    let after_increment = topic_row(&pool, &key).await;
    assert!(state.unread);
    assert_eq!(state.unread_count, 1);
    assert_ne!(before.3, after_increment.3);
    assert_ne!(before.4, after_increment.4);

    let state = set_topic_unread_in_pool(&pool, &key, false, 3)
        .await
        .unwrap();
    let after_read = topic_row(&pool, &key).await;
    assert!(!state.unread);
    assert_eq!(state.unread_count, 0);
    assert_ne!(after_increment.3, after_read.3);
}

#[tokio::test]
async fn 同一消息首次事件重放恢复和_hydration_只产生一笔未读() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "topic-1");
    seed_topic(&pool, &key, false, 0, None).await;
    seed_message(&pool, &key, "same-message").await;

    let first = increment_topic_unread_count_in_pool(&pool, &key, "same-message", false, 1)
        .await
        .unwrap();
    let replay = increment_topic_unread_count_in_pool(&pool, &key, "same-message", true, 2)
        .await
        .unwrap();
    let hydration = increment_topic_unread_count_in_pool(&pool, &key, "same-message", true, 3)
        .await
        .unwrap();

    assert_eq!(first.unread_count, 0);
    assert_eq!(replay.unread_count, 0);
    assert_eq!(hydration.unread_count, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM message_unread_receipts
             WHERE owner_type = 'agent' AND owner_id = 'owner-a'
               AND topic_id = 'topic-1' AND msg_id = 'same-message'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn 未读计数列表忽略群组所有者碰撞() {
    let pool = test_pool().await;
    let agent_key = TopicKey::new("agent", "shared-owner", "agent-topic");
    let group_key = TopicKey::new("group", "shared-owner", "group-topic");
    seed_topic(&pool, &agent_key, true, 3, None).await;
    seed_topic(&pool, &group_key, true, 7, None).await;

    let counts = super::listing::load_unread_counts(&pool).await.unwrap();
    assert!(!counts.contains_key("shared-owner"));
    assert_eq!(counts.get("agent:shared-owner"), Some(&3));
    assert_eq!(counts.get("group:shared-owner"), Some(&7));
}

#[tokio::test]
async fn unread_count_aggregation_does_not_cross_encoded_composite_key() {
    let pool = test_pool().await;
    let group_id = "x/y";
    let agent_id = "group:x%2Fy";
    seed_owner(&pool, "group", group_id).await;
    seed_owner(&pool, "agent", agent_id).await;
    seed_topic(
        &pool,
        &TopicKey::new("group", group_id, "group-topic"),
        true,
        7,
        None,
    )
    .await;
    seed_topic(
        &pool,
        &TopicKey::new("agent", agent_id, "agent-topic"),
        true,
        11,
        None,
    )
    .await;

    let counts = super::listing::load_unread_counts(&pool).await.unwrap();
    assert_eq!(counts.get("group:x%2Fy"), Some(&7));
    assert_eq!(counts.get("agent:group%3Ax%252Fy"), Some(&11));
}

#[tokio::test]
async fn unread_count_aggregation_key_matches_frontend_contract() {
    let pool = test_pool().await;
    let cases = [
        ("owner!x", "agent:owner!x"),
        ("owner with space", "agent:owner%20with%20space"),
        ("owner%value", "agent:owner%25value"),
        ("owner/path", "agent:owner%2Fpath"),
        (
            "所有者/😀",
            "agent:%E6%89%80%E6%9C%89%E8%80%85%2F%F0%9F%98%80",
        ),
    ];
    for (index, (owner_id, expected_key)) in cases.into_iter().enumerate() {
        seed_owner(&pool, "agent", owner_id).await;
        seed_topic(
            &pool,
            &TopicKey::new("agent", owner_id, format!("topic-{index}")),
            true,
            1,
            None,
        )
        .await;
        let counts = super::listing::load_unread_counts(&pool).await.unwrap();
        assert_eq!(counts.get(expected_key), Some(&1));
    }
}
