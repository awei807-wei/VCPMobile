use super::*;
use crate::vcp_modules::db_manager::DbState;
use sqlx::{Row, SqlitePool};
use std::path::PathBuf;
use tauri::Manager;

fn config() -> GroupConfig {
    GroupConfig {
        id: "group-a".to_string(),
        name: "new".to_string(),
        avatar_calculated_color: None,
        members: vec!["agent-new".to_string()],
        mode: "invite_only".to_string(),
        member_tags: Some(serde_json::json!({"agent-new": "new-tag"})),
        group_prompt: Some("prompt".to_string()),
        invite_prompt: Some("invite".to_string()),
        use_unified_model: true,
        unified_model: Some("new-unified".to_string()),
        topics: Vec::new(),
        tag_match_mode: Some("natural".to_string()),
        created_at: 1,
    }
}

async fn pool(with_topic_hydration_columns: bool) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE groups (
            group_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mode TEXT NOT NULL,
            group_prompt TEXT,
            invite_prompt TEXT,
            use_unified_model INTEGER NOT NULL,
            unified_model TEXT,
            tag_match_mode TEXT,
            created_at INTEGER NOT NULL,
            config_hash TEXT NOT NULL,
            content_hash TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            deleted_at INTEGER
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE group_members (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            sort_order INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE group_member_tags (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            member_tag TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    let topic_schema = if with_topic_hydration_columns {
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            locked INTEGER NOT NULL DEFAULT 1,
            unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )"
    } else {
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        )"
    };
    sqlx::query(topic_schema).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO groups (
            group_id, name, mode, group_prompt, invite_prompt,
            use_unified_model, tag_match_mode, created_at, config_hash,
            content_hash, updated_at
         ) VALUES ('group-a', 'old', 'sequential', 'old-prompt', 'old-invite',
                   0, 'strict', 1, 'old-config-hash', 'old-content-hash', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO group_members(group_id, agent_id, sort_order, updated_at)
         VALUES ('group-a', 'agent-old', 0, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO group_member_tags(group_id, agent_id, member_tag, updated_at)
         VALUES ('group-a', 'agent-old', 'old-tag', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    if with_topic_hydration_columns {
        sqlx::query(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, updated_at,
                locked, unread, unread_count, msg_count, config_hash, content_hash
             ) VALUES ('group', 'group-a', 'topic-old', '权威话题', 3, 4, 0, 1, 7, 11,
                       'topic-config-hash', 'topic-content-hash')",
        )
        .execute(&pool)
        .await
        .unwrap();
    } else {
        sqlx::query(
            "INSERT INTO topics (owner_type, owner_id, topic_id, config_hash, content_hash)
             VALUES ('group', 'group-a', 'topic-old', 'topic-config-hash', 'topic-content-hash')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    pool
}

fn test_app(pool: SqlitePool) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool,
        path: PathBuf::from("test.sqlite"),
    });
    app.manage(GroupManagerState::new());
    app
}

fn old_config() -> GroupConfig {
    GroupConfig {
        id: "group-a".to_string(),
        name: "old".to_string(),
        avatar_calculated_color: None,
        members: vec!["agent-old".to_string()],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({"agent-old": "old-tag"})),
        group_prompt: Some("old-prompt".to_string()),
        invite_prompt: Some("old-invite".to_string()),
        use_unified_model: false,
        unified_model: None,
        topics: Vec::new(),
        tag_match_mode: Some("strict".to_string()),
        created_at: 1,
    }
}

async fn assert_group_config_unchanged(pool: &SqlitePool, before: &GroupConfig) {
    let row = sqlx::query(
        "SELECT name, mode, group_prompt, invite_prompt, use_unified_model,
                unified_model, tag_match_mode, config_hash, content_hash
         FROM groups WHERE group_id = 'group-a'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("name"), before.name);
    assert_eq!(row.get::<String, _>("mode"), before.mode);
    assert_eq!(
        row.get::<Option<String>, _>("group_prompt"),
        before.group_prompt
    );
    assert_eq!(
        row.get::<Option<String>, _>("invite_prompt"),
        before.invite_prompt
    );
    assert_eq!(
        row.get::<i64, _>("use_unified_model") != 0,
        before.use_unified_model
    );
    assert_eq!(
        row.get::<Option<String>, _>("unified_model"),
        before.unified_model
    );
    assert_eq!(
        row.get::<Option<String>, _>("tag_match_mode"),
        before.tag_match_mode
    );
    assert_eq!(row.get::<String, _>("config_hash"), "old-config-hash");
    assert_eq!(row.get::<String, _>("content_hash"), "old-content-hash");
}

async fn assert_group_members_and_tags_unchanged(pool: &SqlitePool) {
    let members = sqlx::query(
        "SELECT agent_id, sort_order, updated_at
         FROM group_members WHERE group_id = 'group-a' ORDER BY sort_order",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].get::<String, _>("agent_id"), "agent-old");
    assert_eq!(members[0].get::<i64, _>("sort_order"), 0);
    assert_eq!(members[0].get::<i64, _>("updated_at"), 1);
    let tags = sqlx::query(
        "SELECT agent_id, member_tag, updated_at
         FROM group_member_tags WHERE group_id = 'group-a' ORDER BY agent_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].get::<String, _>("agent_id"), "agent-old");
    assert_eq!(tags[0].get::<String, _>("member_tag"), "old-tag");
    assert_eq!(tags[0].get::<i64, _>("updated_at"), 1);
}

async fn assert_group_member_and_tag_counts(pool: &SqlitePool) {
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM group_members WHERE group_id = 'group-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM group_member_tags WHERE group_id = 'group-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn bubble_failure_rolls_back_group_config_members_and_tags() {
    let app = test_app(pool(false).await);
    sqlx::query(
        "CREATE TRIGGER fail_group_bubble
         BEFORE UPDATE OF content_hash ON groups
         BEGIN SELECT RAISE(ABORT, 'injected group bubble failure'); END",
    )
    .execute(&app.state::<DbState>().pool)
    .await
    .unwrap();

    let before = old_config();
    let error = save_group_config(app.handle().clone(), app.state(), config())
        .await
        .expect_err("injected bubble failure should abort the write");
    assert!(error.contains("injected group bubble failure"), "{error}");
    let pool = &app.state::<DbState>().pool;
    assert_group_config_unchanged(pool, &before).await;
    assert_group_members_and_tags_unchanged(pool).await;
}

#[tokio::test]
async fn topic_hydration_failure_rolls_back_group_config_hash_members_and_tags() {
    let app = test_app(pool(false).await);
    let before = old_config();
    let error = save_group_config(app.handle().clone(), app.state(), config())
        .await
        .expect_err("缺少 hydration 列时保存必须在提交前失败");
    assert!(error.contains("title"), "{error}");
    let pool = &app.state::<DbState>().pool;
    assert_group_config_unchanged(pool, &before).await;
    assert_group_member_and_tag_counts(pool).await;
}

#[path = "group_service_write_topic_tests.rs"]
mod topic_tests;
