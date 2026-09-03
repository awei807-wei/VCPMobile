use super::{set_topic_unread_in_pool, TopicKey};
use sqlx::{Row, SqlitePool};

#[tokio::test]
async fn marking_topic_read_clears_unread_count() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            unread INTEGER NOT NULL,
            unread_count INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, unread, unread_count, updated_at)
         VALUES ('agent', 'owner-a', 'topic_1', 1, 3, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    set_topic_unread_in_pool(
        &pool,
        &TopicKey::new("agent", "owner-a", "topic_1"),
        false,
        2,
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT unread, unread_count, updated_at FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'topic_1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i32, _>("unread"), 0);
    assert_eq!(row.get::<i32, _>("unread_count"), 0);
    assert_eq!(row.get::<i64, _>("updated_at"), 2);
}

#[tokio::test]
async fn marking_topic_unread_preserves_existing_count() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            unread INTEGER NOT NULL,
            unread_count INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, unread, unread_count, updated_at)
         VALUES ('agent', 'owner-a', 'topic_1', 0, 5, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    set_topic_unread_in_pool(
        &pool,
        &TopicKey::new("agent", "owner-a", "topic_1"),
        true,
        2,
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT unread, unread_count, updated_at FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'topic_1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i32, _>("unread"), 1);
    assert_eq!(row.get::<i32, _>("unread_count"), 5);
    assert_eq!(row.get::<i64, _>("updated_at"), 2);
}

#[tokio::test]
async fn same_topic_id_isolated_by_composite_owner_key() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            unread INTEGER NOT NULL,
            unread_count INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, unread, unread_count, updated_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 1, 3, 1),
            ('group', 'owner-g', 'shared-topic', 1, 7, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    set_topic_unread_in_pool(
        &pool,
        &TopicKey::new("agent", "owner-a", "shared-topic"),
        false,
        2,
    )
    .await
    .unwrap();

    let rows = sqlx::query(
        "SELECT owner_type, unread, unread_count, updated_at
         FROM topics WHERE topic_id = 'shared-topic' ORDER BY owner_type",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("owner_type"), "agent");
    assert_eq!(rows[0].get::<i32, _>("unread"), 0);
    assert_eq!(rows[0].get::<i32, _>("unread_count"), 0);
    assert_eq!(rows[0].get::<i64, _>("updated_at"), 2);
    assert_eq!(rows[1].get::<String, _>("owner_type"), "group");
    assert_eq!(rows[1].get::<i32, _>("unread"), 1);
    assert_eq!(rows[1].get::<i32, _>("unread_count"), 7);
    assert_eq!(rows[1].get::<i64, _>("updated_at"), 1);
}
