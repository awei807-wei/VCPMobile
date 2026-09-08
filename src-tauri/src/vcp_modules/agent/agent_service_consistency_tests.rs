use super::{get_assistants_snapshot, read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_service::{
    get_groups, read_group_config_internal, GroupManagerState,
};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use std::path::PathBuf;
use tauri::Manager;

const SHARED_OWNER: &str = "shared-owner";

async fn consistency_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("创建一致性测试数据库失败");
    sqlx::raw_sql(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            system_prompt TEXT NOT NULL,
            mobile_system_prompt TEXT NOT NULL,
            model TEXT NOT NULL,
            temperature REAL NOT NULL,
            context_token_limit INTEGER NOT NULL,
            max_output_tokens INTEGER NOT NULL,
            stream_output INTEGER NOT NULL,
            use_temperature INTEGER NOT NULL,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            deleted_at INTEGER
        );
        CREATE TABLE groups (
            group_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mode TEXT NOT NULL,
            group_prompt TEXT,
            invite_prompt TEXT,
            use_unified_model INTEGER NOT NULL,
            unified_model TEXT,
            tag_match_mode TEXT,
            config_hash TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            deleted_at INTEGER
        );
        CREATE TABLE avatars (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            dominant_color TEXT,
            deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id)
        );
        CREATE TABLE group_members (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            sort_order INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        );
        CREATE TABLE group_member_tags (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            member_tag TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        );
        CREATE TABLE topics (
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
        );
        INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model,
            temperature, context_token_limit, max_output_tokens,
            stream_output, use_temperature, updated_at
        ) VALUES (
            'shared-owner', 'agent-before', 'agent-system', '', 'agent-model',
            1, 100, 20, 1, 1, 1
        );
        INSERT INTO groups (
            group_id, name, mode, group_prompt, invite_prompt,
            use_unified_model, unified_model, tag_match_mode, created_at, updated_at
        ) VALUES (
            'shared-owner', 'group-before', 'sequential', 'group-prompt',
            'invite-prompt', 0, NULL, 'strict', 1, 1
        );
        INSERT INTO avatars (owner_type, owner_id, dominant_color)
        VALUES ('agent', 'shared-owner', '#111111'), ('group', 'shared-owner', '#222222');
        INSERT INTO group_members (group_id, agent_id, sort_order, updated_at)
        VALUES ('shared-owner', 'agent-before', 0, 1);
        INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
        VALUES ('shared-owner', 'agent-before', 'tag-before', 1);
        INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            locked, unread, unread_count, msg_count
        ) VALUES
            ('agent', 'shared-owner', 'agent-topic', 'agent-topic-before', 1, 1,
             1, 0, 0, 1),
            ('group', 'shared-owner', 'group-topic', 'group-topic-before', 1, 1,
             1, 0, 0, 1);",
    )
    .execute(&pool)
    .await
    .expect("创建一致性测试数据库结构失败");
    pool
}

fn test_app(pool: SqlitePool) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool,
        path: PathBuf::from("task15-consistency.sqlite"),
    });
    app.manage(AgentConfigState::new());
    app.manage(GroupManagerState::new());
    app
}

#[tokio::test]
async fn config_reads_reflect_external_updates_and_preserve_owner_type_identity() {
    let app = test_app(consistency_pool().await);
    let agent_state = app.state::<AgentConfigState>();
    let group_state = app.state::<GroupManagerState>();
    let initial_agent = read_agent_config_internal(
        &app.handle().clone(),
        &agent_state,
        SHARED_OWNER,
        Some(false),
    )
    .await
    .expect("应读取初始 Agent 配置");
    let initial_group =
        read_group_config_internal(&app.handle().clone(), &group_state, SHARED_OWNER)
            .await
            .expect("应读取初始 Group 配置");
    assert_eq!(initial_agent.name, "agent-before");
    assert_eq!(
        initial_agent.avatar_calculated_color.as_deref(),
        Some("#111111")
    );
    assert_eq!(initial_agent.topics[0].owner_type, "agent");
    assert_eq!(initial_group.name, "group-before");
    assert_eq!(
        initial_group.avatar_calculated_color.as_deref(),
        Some("#222222")
    );
    assert_eq!(initial_group.topics[0].owner_type, "group");

    let pool = &app.state::<DbState>().pool;
    let mut tx = pool.begin().await.expect("应开始外部更新事务");
    sqlx::query("UPDATE agents SET name = 'agent-after', system_prompt = 'agent-system-after' WHERE agent_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE groups SET name = 'group-after' WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE avatars SET dominant_color = CASE owner_type WHEN 'agent' THEN '#aaaaaa' ELSE '#bbbbbb' END WHERE owner_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, agent_id, sort_order, updated_at) VALUES (?, 'agent-after', 0, 2)")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM group_member_tags WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at) VALUES (?, 'agent-after', 'tag-after', 2)")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET title = CASE owner_type WHEN 'agent' THEN 'agent-topic-after' ELSE 'group-topic-after' END, unread = 1, unread_count = 7, updated_at = 2 WHERE owner_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.expect("外部更新事务应提交");

    let updated_agent = read_agent_config_internal(
        &app.handle().clone(),
        &agent_state,
        SHARED_OWNER,
        Some(false),
    )
    .await
    .expect("应读取外部更新后的 Agent 配置");
    let updated_group =
        read_group_config_internal(&app.handle().clone(), &group_state, SHARED_OWNER)
            .await
            .expect("应读取外部更新后的 Group 配置");
    assert_eq!(updated_agent.name, "agent-after");
    assert_eq!(updated_agent.system_prompt, "agent-system-after");
    assert_eq!(
        updated_agent.avatar_calculated_color.as_deref(),
        Some("#aaaaaa")
    );
    assert_eq!(updated_agent.topics[0].name, "agent-topic-after");
    assert_eq!(updated_agent.topics[0].owner_type, "agent");
    assert_eq!(updated_group.name, "group-after");
    assert_eq!(
        updated_group.avatar_calculated_color.as_deref(),
        Some("#bbbbbb")
    );
    assert_eq!(updated_group.members, vec!["agent-after"]);
    assert_eq!(
        updated_group.member_tags,
        Some(serde_json::json!({"agent-after": "tag-after"}))
    );
    assert_eq!(updated_group.topics[0].name, "group-topic-after");
    assert_eq!(updated_group.topics[0].owner_type, "group");
    assert_eq!(updated_group.topics[0].unread_count, 7);
}

#[tokio::test]
async fn group_listing_reads_one_committed_snapshot_of_group_members_tags_and_avatar() {
    let app = test_app(consistency_pool().await);
    let initial = get_groups(app.handle().clone(), app.state())
        .await
        .expect("应读取初始 Group 列表");
    assert_eq!(initial[0].name, "group-before");
    assert_eq!(initial[0].members, vec!["agent-before"]);
    assert_eq!(
        initial[0].avatar_calculated_color.as_deref(),
        Some("#222222")
    );

    let pool = &app.state::<DbState>().pool;
    let mut tx = pool.begin().await.expect("应开始 Group 列表更新事务");
    sqlx::query("UPDATE groups SET name = 'group-list-after' WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE avatars SET dominant_color = '#cccccc' WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(SHARED_OWNER)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, agent_id, sort_order, updated_at) VALUES (?, 'agent-list-after', 0, 3)")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM group_member_tags WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at) VALUES (?, 'agent-list-after', 'tag-list-after', 3)")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.expect("Group 列表更新事务应提交");

    let updated = get_groups(app.handle().clone(), app.state())
        .await
        .expect("应读取更新后的 Group 列表");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].name, "group-list-after");
    assert_eq!(updated[0].members, vec!["agent-list-after"]);
    assert_eq!(
        updated[0].avatar_calculated_color.as_deref(),
        Some("#cccccc")
    );
}

#[tokio::test]
async fn assistants_snapshot_reads_agent_group_and_unread_updates_from_one_transaction() {
    let app = test_app(consistency_pool().await);
    let initial = get_assistants_snapshot(app.state(), app.state(), app.state())
        .await
        .expect("应读取初始 Assistant 快照");
    assert_eq!(initial.agents[0].name, "agent-before");
    assert_eq!(initial.groups[0].name, "group-before");
    assert!(!initial.unread_counts.contains_key(SHARED_OWNER));

    let pool = &app.state::<DbState>().pool;
    let mut tx = pool.begin().await.expect("应开始 Assistant 快照更新事务");
    sqlx::query("UPDATE agents SET name = 'agent-snapshot-after' WHERE agent_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE groups SET name = 'group-snapshot-after' WHERE group_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE avatars SET dominant_color = CASE owner_type WHEN 'agent' THEN '#dddddd' ELSE '#eeeeee' END WHERE owner_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET unread = 1, unread_count = 4 WHERE owner_type = 'agent' AND owner_id = ?")
        .bind(SHARED_OWNER)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.expect("Assistant 快照更新事务应提交");

    let updated = get_assistants_snapshot(app.state(), app.state(), app.state())
        .await
        .expect("应读取更新后的 Assistant 快照");
    assert_eq!(updated.agents[0].name, "agent-snapshot-after");
    assert_eq!(
        updated.agents[0].avatar_calculated_color.as_deref(),
        Some("#dddddd")
    );
    assert_eq!(updated.groups[0].name, "group-snapshot-after");
    assert_eq!(
        updated.groups[0].avatar_calculated_color.as_deref(),
        Some("#eeeeee")
    );
    assert_eq!(updated.unread_counts.get("agent:shared-owner"), Some(&4));
    assert!(!updated.unread_counts.contains_key(SHARED_OWNER));
}

#[tokio::test]
async fn soft_deleted_avatar_is_not_returned_by_agent_or_group_snapshots() {
    let app = test_app(consistency_pool().await);
    let handle = app.handle().clone();
    crate::vcp_modules::sync_executor::DeleteExecutor::soft_delete_avatar(
        &handle,
        "agent",
        SHARED_OWNER,
        42,
    )
    .await
    .expect("真实头像软删除应成功");

    let agent = read_agent_config_internal(
        &handle,
        &app.state::<AgentConfigState>(),
        SHARED_OWNER,
        Some(false),
    )
    .await
    .expect("软删除头像后 Agent 仍应可读取");
    assert_eq!(agent.avatar_calculated_color, None);

    let assistants = get_assistants_snapshot(
        app.state::<AgentConfigState>(),
        app.state::<GroupManagerState>(),
        app.state(),
    )
    .await
    .expect("软删除头像后 Assistant 快照仍应可读取");
    assert_eq!(assistants.agents[0].avatar_calculated_color, None);
    assert_eq!(
        assistants.groups[0].avatar_calculated_color.as_deref(),
        Some("#222222")
    );

    let groups = get_groups(handle, app.state())
        .await
        .expect("软删除头像后 Group 列表仍应可读取");
    assert_eq!(
        groups[0].avatar_calculated_color.as_deref(),
        Some("#222222")
    );
}
