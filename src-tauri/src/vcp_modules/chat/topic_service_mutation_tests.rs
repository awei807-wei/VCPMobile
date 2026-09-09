use super::mutations::{
    create_topic_in_pool, delete_topic_with_notify, toggle_topic_lock_in_pool,
    update_topic_title_in_pool, CommitMode,
};
use super::tests::{seed_message, seed_topic, test_pool};
use super::TopicKey;
use crate::vcp_modules::topic_types::Topic;
use sqlx::{Row, SqlitePool};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

async fn assert_created_topic(
    pool: &SqlitePool,
    topic: &Topic,
    owner_type: &str,
    owner_id: &str,
    name: &str,
    now: i64,
) {
    let row: (String, String, String, String, i64, String, String) = sqlx::query_as(
        "SELECT owner_type, owner_id, topic_id, title, created_at, config_hash, content_hash
         FROM topics WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(&topic.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        (row.0, row.1, row.2, row.3, row.4),
        (
            owner_type.to_string(),
            owner_id.to_string(),
            topic.id.clone(),
            name.to_string(),
            now,
        )
    );
    assert_eq!(row.5.len(), 64);
    assert!(row.6.is_empty());
}

async fn assert_owner_hash(pool: &SqlitePool, owner_type: &str, owner_id: &str) {
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
    let query = format!("SELECT content_hash FROM {table} WHERE {column} = ?");
    let hash: String = sqlx::query_scalar(&query)
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(hash.len(), 64);
}

async fn deleted_at(pool: &SqlitePool, key: &TopicKey, table: &str, id: &str) -> Option<i64> {
    let query = match table {
        "topics" => {
            "SELECT deleted_at FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
        }
        "messages" => {
            "SELECT deleted_at FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?"
        }
        _ => unreachable!("unsupported deletion table"),
    };
    let mut statement = sqlx::query(query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    if table == "messages" {
        statement = statement.bind(id);
    }
    statement
        .fetch_one(pool)
        .await
        .unwrap()
        .try_get("deleted_at")
        .unwrap()
}

async fn count_topic_rows(pool: &SqlitePool, table: &str, key: &TopicKey) -> i64 {
    let query = format!(
        "SELECT COUNT(*) FROM {table}
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
    );
    sqlx::query_scalar(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn assert_delete_rollback(
    pool: &SqlitePool,
    key: &TopicKey,
    before_hash: &str,
    notifications: &Arc<AtomicUsize>,
) {
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
    assert_eq!(deleted_at(pool, key, "topics", "").await, None);
    let owner_hash: String =
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'owner-a'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(owner_hash, before_hash);
    assert_eq!(deleted_at(pool, key, "messages", "message-1").await, None);
    assert_eq!(count_topic_rows(pool, "message_attachments", key).await, 1);
    assert_eq!(count_topic_rows(pool, "active_generations", key).await, 1);
}

async fn assert_delete_committed(
    pool: &SqlitePool,
    key: &TopicKey,
    remaining: &TopicKey,
    deleted_at_value: i64,
    before_hash: &str,
    notifications: &Arc<AtomicUsize>,
) {
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert_eq!(
        deleted_at(pool, key, "topics", "").await,
        Some(deleted_at_value)
    );
    assert_eq!(
        deleted_at(pool, key, "messages", "message-1").await,
        Some(deleted_at_value)
    );
    assert_eq!(count_topic_rows(pool, "message_attachments", key).await, 0);
    assert_eq!(count_topic_rows(pool, "active_generations", key).await, 0);
    let owner_hash: String =
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'owner-a'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_ne!(owner_hash, before_hash);
    assert_eq!(owner_hash.len(), 64);
    assert_eq!(deleted_at(pool, remaining, "topics", "").await, None);
}

#[tokio::test]
async fn 同所有者同毫秒创建普通和群组话题仍生成唯一有效记录() {
    let pool = test_pool().await;
    let now = 99;
    let first_agent = create_topic_in_pool(
        &pool,
        "owner-a".to_string(),
        "agent".to_string(),
        "一".to_string(),
        now,
    )
    .await
    .unwrap();
    let second_agent = create_topic_in_pool(
        &pool,
        "owner-a".to_string(),
        "agent".to_string(),
        "二".to_string(),
        now,
    )
    .await
    .unwrap();
    let first_group = create_topic_in_pool(
        &pool,
        "shared-owner".to_string(),
        "group".to_string(),
        "三".to_string(),
        now,
    )
    .await
    .unwrap();
    let second_group = create_topic_in_pool(
        &pool,
        "shared-owner".to_string(),
        "group".to_string(),
        "四".to_string(),
        now,
    )
    .await
    .unwrap();

    assert_ne!(first_agent.id, second_agent.id);
    assert_ne!(first_group.id, second_group.id);
    assert!(first_agent.id.starts_with("topic_99_"));
    assert!(first_group.id.starts_with("group_topic_99_"));
    assert_created_topic(&pool, &first_agent, "agent", "owner-a", "一", now).await;
    assert_created_topic(&pool, &second_agent, "agent", "owner-a", "二", now).await;
    assert_created_topic(&pool, &first_group, "group", "shared-owner", "三", now).await;
    assert_created_topic(&pool, &second_group, "group", "shared-owner", "四", now).await;
    assert_owner_hash(&pool, "agent", "owner-a").await;
    assert_owner_hash(&pool, "group", "shared-owner").await;
}

#[tokio::test]
async fn 创建话题哈希失败时话题和所有者均回滚() {
    let pool = test_pool().await;
    sqlx::query(
        "CREATE TRIGGER fail_topic_bubble
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected topic bubble failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = create_topic_in_pool(
        &pool,
        "owner-a".to_string(),
        "agent".to_string(),
        "新话题".to_string(),
        10,
    )
    .await;
    let error = result.expect_err("话题哈希失败应回滚创建");
    assert!(error.contains("injected topic bubble failure"), "{error}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT content_hash FROM agents WHERE agent_id = 'owner-a'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        ""
    );
}

#[tokio::test]
async fn 更新话题标题哈希失败时标题和哈希均回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "title-topic");
    seed_topic(&pool, &key, false, 0, None).await;
    sqlx::query(
        "CREATE TRIGGER fail_topic_bubble
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected topic bubble failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = update_topic_title_in_pool(&pool, &key, "新标题", 20)
        .await
        .expect_err("标题哈希失败应回滚更新");
    assert!(error.contains("injected topic bubble failure"), "{error}");
    let row: (String, i64, String, String) = sqlx::query_as(
        "SELECT title, updated_at, config_hash, content_hash FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "测试话题".to_string(),
            1,
            "before-config".to_string(),
            "before-content".to_string()
        )
    );
}

#[tokio::test]
async fn 更新话题锁状态哈希失败时锁和哈希均回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "lock-topic");
    seed_topic(&pool, &key, false, 0, None).await;
    sqlx::query(
        "CREATE TRIGGER fail_topic_bubble
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected topic bubble failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = toggle_topic_lock_in_pool(&pool, &key, false, 20)
        .await
        .expect_err("锁状态哈希失败应回滚更新");
    assert!(error.contains("injected topic bubble failure"), "{error}");
    let row: (i64, i64, String, String) = sqlx::query_as(
        "SELECT locked, updated_at, config_hash, content_hash FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            1,
            1,
            "before-config".to_string(),
            "before-content".to_string()
        )
    );
}

#[tokio::test]
async fn 删除话题哈希失败时业务数据和通知均回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "delete-topic");
    seed_topic(&pool, &key, false, 0, None).await;
    seed_message(&pool, &key, "message-1").await;
    sqlx::query(
        "CREATE TRIGGER fail_owner_bubble
         BEFORE UPDATE OF content_hash ON agents
         BEGIN SELECT RAISE(ABORT, 'injected owner bubble failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let notifications = Arc::new(AtomicUsize::new(0));
    let result = delete_topic_with_notify(&pool, &key, 30, CommitMode::Real, {
        let notifications = Arc::clone(&notifications);
        move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;
    let error = result.expect_err("所有者哈希失败应回滚删除");
    assert!(error.contains("injected owner bubble failure"), "{error}");
    assert_delete_rollback(&pool, &key, "", &notifications).await;
}

#[tokio::test]
async fn 删除话题提交失败时数据哈希未提交且不发送通知() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "commit-topic");
    seed_topic(&pool, &key, false, 0, None).await;
    seed_message(&pool, &key, "message-1").await;
    let before_hash: String =
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'owner-a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let notifications = Arc::new(AtomicUsize::new(0));
    let result = delete_topic_with_notify(&pool, &key, 40, CommitMode::Fail, {
        let notifications = Arc::clone(&notifications);
        move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;
    let error = result.expect_err("真实 SQLite COMMIT 阶段应因延迟外键失败");
    assert!(error.contains("FOREIGN KEY constraint failed"), "{error}");
    assert_delete_rollback(&pool, &key, &before_hash, &notifications).await;
}

#[tokio::test]
async fn 删除话题提交成功后业务和所有者哈希已提交且通知一次() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "committed-topic");
    let remaining = TopicKey::new("agent", "owner-a", "remaining-topic");
    seed_topic(&pool, &key, false, 0, None).await;
    seed_message(&pool, &key, "message-1").await;
    seed_topic(&pool, &remaining, false, 0, None).await;
    let before_hash: String =
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'owner-a'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let notifications = Arc::new(AtomicUsize::new(0));
    delete_topic_with_notify(&pool, &key, 50, CommitMode::Real, {
        let notifications = Arc::clone(&notifications);
        move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await
    .expect("删除话题提交");
    assert_delete_committed(&pool, &key, &remaining, 50, &before_hash, &notifications).await;
}
