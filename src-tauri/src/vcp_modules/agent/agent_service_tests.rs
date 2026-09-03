use sqlx::{Row, SqlitePool};

#[tokio::test]
async fn agent_topic_upsert_and_delete_are_owner_scoped() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            locked INTEGER NOT NULL,
            unread INTEGER NOT NULL,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE active_generations (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, title, created_at, updated_at, locked, unread)
         VALUES
            ('group', 'shared-owner', 'shared-topic', 'group', 1, 1, 1, 0),
            ('agent', 'shared-owner', 'shared-topic', 'old', 1, 1, 1, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO messages (owner_type, owner_id, topic_id, msg_id)
         VALUES
            ('group', 'shared-owner', 'shared-topic', 'group-msg'),
            ('agent', 'shared-owner', 'shared-topic', 'agent-msg')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO active_generations (owner_type, owner_id, topic_id, msg_id)
         VALUES
            ('group', 'shared-owner', 'shared-topic', 'group-msg'),
            ('agent', 'shared-owner', 'shared-topic', 'agent-msg')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO topics
            (topic_id, owner_type, owner_id, title, created_at, updated_at, locked, unread)
         VALUES (?, 'agent', ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
            title = excluded.title, locked = excluded.locked,
            unread = excluded.unread, updated_at = excluded.updated_at",
    )
    .bind("shared-topic")
    .bind("shared-owner")
    .bind("agent-updated")
    .bind(2_i64)
    .bind(2_i64)
    .bind(false)
    .bind(true)
    .execute(&pool)
    .await
    .unwrap();
    let now = 3_i64;
    sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind("shared-owner")
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind("shared-owner")
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind("shared-owner")
    .execute(&pool)
    .await
    .unwrap();

    let group_topic = sqlx::query(
        "SELECT title, deleted_at FROM topics
         WHERE owner_type = 'group' AND owner_id = 'shared-owner' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(group_topic.get::<String, _>("title"), "group");
    assert!(group_topic.get::<Option<i64>, _>("deleted_at").is_none());
    let group_message = sqlx::query(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'shared-owner' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(group_message.get::<Option<i64>, _>("deleted_at").is_none());
    let group_generation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM active_generations
         WHERE owner_type = 'group' AND owner_id = 'shared-owner' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(group_generation_count, 1);
}
