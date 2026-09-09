use super::{pool, test_app};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::Topic;
use sqlx::{Row, SqlitePool};
use tauri::Manager;

#[derive(Debug, PartialEq, Eq)]
struct TopicSnapshot {
    owner_type: String,
    owner_id: String,
    topic_id: String,
    title: String,
    created_at: i64,
    updated_at: i64,
    locked: i64,
    unread: i64,
    unread_count: i64,
    msg_count: i64,
    config_hash: String,
    content_hash: String,
    deleted_at: Option<i64>,
}

async fn topic_snapshot(pool: &SqlitePool) -> TopicSnapshot {
    let row = sqlx::query(
        "SELECT owner_type, owner_id, topic_id, title, created_at, updated_at,
                locked, unread, unread_count, msg_count, config_hash, content_hash,
                deleted_at
         FROM topics WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'topic-old'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    TopicSnapshot {
        owner_type: row.get("owner_type"),
        owner_id: row.get("owner_id"),
        topic_id: row.get("topic_id"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        locked: row.get("locked"),
        unread: row.get("unread"),
        unread_count: row.get("unread_count"),
        msg_count: row.get("msg_count"),
        config_hash: row.get("config_hash"),
        content_hash: row.get("content_hash"),
        deleted_at: row.get("deleted_at"),
    }
}

async fn prepare_agent_update(pool: &SqlitePool) -> TopicSnapshot {
    sqlx::query(
        "CREATE TABLE avatars (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, dominant_color TEXT,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id)
        )",
    )
    .execute(pool)
    .await
    .unwrap();
    topic_snapshot(pool).await
}

fn assert_agent_authoritative_topic(topic: &Topic, before: &TopicSnapshot) {
    assert_eq!(topic.id, before.topic_id);
    assert_eq!(topic.name, before.title);
    assert_eq!(topic.created_at, before.created_at);
    assert_eq!(topic.locked, before.locked != 0);
    assert_eq!(topic.unread, before.unread != 0);
    assert_eq!(topic.unread_count, before.unread_count as i32);
    assert_eq!(topic.msg_count, before.msg_count as i32);
    assert_eq!(topic.owner_id, before.owner_id);
    assert_eq!(topic.owner_type, before.owner_type);
}

#[tokio::test]
async fn update_agent_config_uses_authoritative_topics_for_stale_input() {
    let app = test_app(pool(true).await);
    let db_pool = &app.state::<DbState>().pool;
    let before = prepare_agent_update(db_pool).await;
    let updated = super::super::update_agent_config(
        app.handle().clone(),
        app.state(),
        "agent-a".to_string(),
        serde_json::json!({
            "name": "公开 update Agent",
            "topics": [{
                "id": "evil-topic",
                "name": "恶意陈旧快照",
                "createdAt": 999,
                "locked": true,
                "unread": true,
                "unreadCount": 999,
                "msgCount": 999,
                "ownerId": "group-other",
                "ownerType": "group"
            }]
        }),
    )
    .await
    .expect("公开 update Agent 应成功");

    assert_eq!(updated.name, "公开 update Agent");
    assert_eq!(updated.topics.len(), 1);
    let topic = &updated.topics[0];
    assert_agent_authoritative_topic(topic, &before);

    assert_eq!(topic_snapshot(db_pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM topics WHERE owner_type = 'agent' AND owner_id = 'agent-a'",
        )
        .fetch_one(db_pool)
        .await
        .unwrap(),
        1
    );
}
