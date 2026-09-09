use super::{config, pool, test_app};
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
         FROM topics WHERE owner_type = 'group' AND owner_id = 'group-a'
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

async fn prepare_group_update(pool: &SqlitePool) -> TopicSnapshot {
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

fn assert_group_authoritative_topic(topic: &Topic, before: &TopicSnapshot) {
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

async fn assert_saved_group_topic_row(pool: &SqlitePool, before: &sqlx::sqlite::SqliteRow) {
    let after_topic = sqlx::query(
        "SELECT topic_id, title, created_at, updated_at, locked, unread, unread_count,
                msg_count, config_hash, content_hash
         FROM topics WHERE owner_type = 'group' AND owner_id = 'group-a'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(after_topic.get::<String, _>("topic_id"), "topic-old");
    assert_eq!(
        after_topic.get::<String, _>("title"),
        before.get::<String, _>("title")
    );
    for field in [
        "created_at",
        "updated_at",
        "locked",
        "unread",
        "unread_count",
        "msg_count",
    ] {
        assert_eq!(
            after_topic.get::<i64, _>(field),
            before.get::<i64, _>(field),
            "{field}"
        );
    }
    assert_eq!(
        after_topic.get::<String, _>("config_hash"),
        before.get::<String, _>("config_hash")
    );
    assert_eq!(
        after_topic.get::<String, _>("content_hash"),
        before.get::<String, _>("content_hash")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics")
            .fetch_one(pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn save_group_config_uses_authoritative_topics_without_writing_input_snapshot() {
    let app = test_app(pool(true).await);
    let before_topic = sqlx::query(
        "SELECT title, created_at, updated_at, locked, unread, unread_count, msg_count,
                config_hash, content_hash
         FROM topics WHERE owner_type = 'group' AND owner_id = 'group-a'
           AND topic_id = 'topic-old'",
    )
    .fetch_one(&app.state::<crate::vcp_modules::db_manager::DbState>().pool)
    .await
    .unwrap();
    let mut incoming = config();
    incoming.topics = vec![Topic {
        id: "evil-topic".to_string(),
        name: "恶意快照".to_string(),
        created_at: 999,
        locked: true,
        unread: true,
        unread_count: 99,
        msg_count: 99,
        owner_id: "agent-other".to_string(),
        owner_type: "agent".to_string(),
    }];

    assert!(
        super::super::save_group_config(app.handle().clone(), app.state(), incoming,)
            .await
            .expect("保存 Group 应成功")
    );
    assert_saved_group_topic_row(
        &app.state::<crate::vcp_modules::db_manager::DbState>().pool,
        &before_topic,
    )
    .await;
}

#[tokio::test]
async fn update_group_config_uses_authoritative_topics_for_stale_input() {
    let app = test_app(pool(true).await);
    let db_pool = &app.state::<crate::vcp_modules::db_manager::DbState>().pool;
    let before = prepare_group_update(db_pool).await;
    let updated = super::super::update_group_config(
        app.handle().clone(),
        app.state(),
        "group-a".to_string(),
        serde_json::json!({
            "name": "公开 update Group",
            "topics": [{
                "id": "evil-topic",
                "name": "恶意陈旧快照",
                "createdAt": 999,
                "locked": true,
                "unread": true,
                "unreadCount": 999,
                "msgCount": 999,
                "ownerId": "agent-other",
                "ownerType": "agent"
            }]
        }),
    )
    .await
    .expect("公开 update Group 应成功");

    assert_eq!(updated.name, "公开 update Group");
    assert_eq!(updated.topics.len(), 1);
    let topic = &updated.topics[0];
    assert_group_authoritative_topic(topic, &before);

    assert_eq!(topic_snapshot(db_pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM topics WHERE owner_type = 'group' AND owner_id = 'group-a'",
        )
        .fetch_one(db_pool)
        .await
        .unwrap(),
        1
    );
}
