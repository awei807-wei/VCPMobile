use super::{create_group, delete_group, read_group_config, save_group_config, GroupManagerState};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::PathBuf;
use tauri::Manager;

async fn test_pool(with_attachments: bool) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    sqlx::query(
        "CREATE TABLE avatars (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, dominant_color TEXT,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id)
        );
        CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, name TEXT NOT NULL, mode TEXT NOT NULL,
            group_prompt TEXT, invite_prompt TEXT, use_unified_model INTEGER NOT NULL,
            unified_model TEXT, tag_match_mode TEXT, config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL, deleted_at INTEGER
        );
        CREATE TABLE group_members (
            group_id TEXT NOT NULL, agent_id TEXT NOT NULL, member_tag TEXT,
            sort_order INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        );
        CREATE TABLE group_member_tags (
            group_id TEXT NOT NULL, agent_id TEXT NOT NULL, member_tag TEXT NOT NULL,
            updated_at INTEGER NOT NULL, PRIMARY KEY (group_id, agent_id)
        );
        CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0, locked INTEGER NOT NULL DEFAULT 1,
            unread INTEGER NOT NULL DEFAULT 0, unread_count INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0, config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '', deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id)
        );
        CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL,
            timestamp INTEGER NOT NULL, content_hash TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL, deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        );
        CREATE TABLE active_generations (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        );",
    )
    .execute(&pool)
    .await
    .expect("创建 Group 删除测试表失败");
    if with_attachments {
        sqlx::query(
            "CREATE TABLE message_attachments (
                owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
                display_name TEXT NOT NULL, created_at INTEGER NOT NULL,
                PRIMARY KEY (owner_type, owner_id, topic_id, msg_id, attachment_order)
            )",
        )
        .execute(&pool)
        .await
        .expect("创建附件关系表失败");
    }
    sqlx::query(
        "INSERT INTO groups (
            group_id, name, mode, use_unified_model, created_at, updated_at
         ) VALUES ('group-a', '原始群组', 'sequential', 0, 1, 1);
         INSERT INTO group_members (group_id, agent_id, sort_order, updated_at)
         VALUES ('group-a', 'agent-a', 0, 1);
         INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
         VALUES ('group-a', 'agent-a', '标签', 1);
         INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at
         ) VALUES ('group', 'group-a', 'topic-a', '话题', 1, 1);
         INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, content, timestamp, created_at, updated_at
         ) VALUES ('group', 'group-a', 'topic-a', 'msg-a', 'user', '消息', 1, 1, 1);
         INSERT INTO active_generations (
            owner_type, owner_id, topic_id, msg_id, created_at
         ) VALUES ('group', 'group-a', 'topic-a', 'msg-a', 1);",
    )
    .execute(&pool)
    .await
    .expect("写入 Group 删除测试数据失败");
    if with_attachments {
        sqlx::query(
            "INSERT INTO message_attachments (
                owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                display_name, created_at
             ) VALUES ('group', 'group-a', 'topic-a', 'msg-a', 'hash-a', 0, 'a.bin', 1)",
        )
        .execute(&pool)
        .await
        .expect("写入附件关系样例失败");
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

fn stale_group() -> GroupConfig {
    GroupConfig {
        id: "group-a".to_string(),
        name: "过期保存".to_string(),
        avatar_calculated_color: None,
        members: vec!["agent-a".to_string()],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({"agent-a": "标签"})),
        group_prompt: None,
        invite_prompt: None,
        use_unified_model: false,
        unified_model: None,
        topics: vec![],
        tag_match_mode: Some("strict".to_string()),
        created_at: 1,
    }
}

#[path = "group_service_lifecycle_creation_tests.rs"]
mod creation_tests;

#[path = "group_service_lifecycle_delete_tests.rs"]
mod delete_tests;
