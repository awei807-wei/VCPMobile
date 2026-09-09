use super::{
    create_agent, create_default_config, delete_agent, save_agent_config, AgentConfigState,
};
use crate::vcp_modules::db_manager::DbState;
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
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, name TEXT NOT NULL,
            system_prompt TEXT NOT NULL, mobile_system_prompt TEXT NOT NULL,
            model TEXT NOT NULL, temperature REAL NOT NULL,
            context_token_limit INTEGER NOT NULL, max_output_tokens INTEGER NOT NULL,
            stream_output INTEGER NOT NULL, use_temperature INTEGER NOT NULL,
            config_hash TEXT NOT NULL DEFAULT '', content_hash TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL, deleted_at INTEGER
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
    .expect("创建 Agent 删除测试表失败");
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
        "INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model, temperature,
            context_token_limit, max_output_tokens, stream_output, use_temperature, updated_at
         ) VALUES ('agent-a', '原始 Agent', '旧提示词', '', 'model', 1, 100, 100, 1, 0, 1);
         INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at
         ) VALUES ('agent', 'agent-a', 'topic-a', '话题', 1, 1);
         INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, content, timestamp, created_at, updated_at
         ) VALUES ('agent', 'agent-a', 'topic-a', 'msg-a', 'user', '消息', 1, 1, 1);
         INSERT INTO active_generations (
            owner_type, owner_id, topic_id, msg_id, created_at
         ) VALUES ('agent', 'agent-a', 'topic-a', 'msg-a', 1);",
    )
    .execute(&pool)
    .await
    .expect("写入 Agent 删除测试数据失败");
    if with_attachments {
        sqlx::query(
            "INSERT INTO message_attachments (
                owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                display_name, created_at
             ) VALUES ('agent', 'agent-a', 'topic-a', 'msg-a', 'hash-a', 0, 'a.bin', 1)",
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
    app.manage(AgentConfigState::new());
    app
}

async fn assert_agent_delete_rows(pool: &SqlitePool) -> Option<i64> {
    let deleted_at: Option<i64> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'")
            .fetch_one(pool)
            .await
            .expect("读取 Agent 墓碑失败");
    assert!(deleted_at.is_some());
    let topic_deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM topics
         WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'topic-a'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 Agent 话题墓碑失败");
    let message_deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'topic-a' AND msg_id = 'msg-a'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 Agent 消息墓碑失败");
    assert!(topic_deleted_at.is_some());
    assert!(message_deleted_at.is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM active_generations
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM message_attachments WHERE owner_id = 'agent-a'",
        )
        .fetch_one(pool)
        .await
        .unwrap(),
        0
    );
    deleted_at
}

#[tokio::test]
async fn create_agent_bubble_failure_rolls_back_agent_and_initial_topic() {
    let app = test_app(test_pool(false).await);
    sqlx::query(
        "CREATE TRIGGER fail_agent_bubble
         BEFORE UPDATE ON agents
         BEGIN SELECT RAISE(ABORT, 'injected agent bubble failure'); END",
    )
    .execute(&app.state::<DbState>().pool)
    .await
    .unwrap();

    let result = create_agent(
        app.handle().clone(),
        app.state(),
        "新增 Agent".to_string(),
        None,
    )
    .await;
    let error = result.expect_err("agent bubble failure should abort creation");
    assert!(error.contains("injected agent bubble failure"), "{error}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agents")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap(),
        1,
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics")
            .fetch_one(&app.state::<DbState>().pool)
            .await
            .unwrap(),
        1,
    );
}

#[tokio::test]
async fn direct_delete_is_atomic_and_fail_closed() {
    let app = test_app(test_pool(true).await);
    let pool = &app.state::<DbState>().pool;

    delete_agent(app.handle().clone(), app.state(), "agent-a".to_string())
        .await
        .expect("删除存活 Agent 应成功");
    let deleted_at = assert_agent_delete_rows(pool).await;

    let duplicate = delete_agent(app.handle().clone(), app.state(), "agent-a".to_string()).await;
    assert!(duplicate.is_err(), "重复删除必须 fail-closed");
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        deleted_at
    );
    let missing = delete_agent(app.handle().clone(), app.state(), "missing".to_string()).await;
    assert!(missing.is_err(), "缺失 Agent 删除必须 fail-closed");
}

#[tokio::test]
async fn direct_delete_rolls_back_before_relation_cleanup() {
    let app = test_app(test_pool(false).await);
    let error = delete_agent(app.handle().clone(), app.state(), "agent-a".to_string())
        .await
        .expect_err("附件关系表缺失时删除必须回滚");
    assert!(error.contains("message_attachments"));
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM topics
             WHERE owner_type = 'agent' AND owner_id = 'agent-a' AND topic_id = 'topic-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM messages
             WHERE owner_type = 'agent' AND owner_id = 'agent-a'
               AND topic_id = 'topic-a' AND msg_id = 'msg-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM active_generations WHERE owner_id = 'agent-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn save_and_delete_barrier_cannot_revive_agent() {
    let app = test_app(test_pool(true).await);
    let state = app.state::<AgentConfigState>();
    let owner_lock = state.acquire_lock("agent-a").await;
    let owner_guard = owner_lock.lock().await;
    let delete_app: &'static tauri::AppHandle<tauri::test::MockRuntime> =
        Box::leak(Box::new(app.handle().clone()));
    let save_app = delete_app.clone();
    let stale = create_default_config("agent-a");
    let delete_task = tokio::spawn(async move {
        delete_agent(
            delete_app.clone(),
            delete_app.state(),
            "agent-a".to_string(),
        )
        .await
    });
    let save_task =
        tokio::spawn(
            async move { save_agent_config(save_app.clone(), save_app.state(), stale).await },
        );
    tokio::task::yield_now().await;
    assert!(!delete_task.is_finished() && !save_task.is_finished());
    drop(owner_guard);
    let _ = delete_task.await.expect("删除任务 panic");
    let _ = save_task.await.expect("保存任务 panic");
    assert!(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT deleted_at FROM agents WHERE agent_id = 'agent-a'",
        )
        .fetch_one(&app.state::<DbState>().pool)
        .await
        .unwrap()
        .is_some(),
        "save/delete barrier 后 Agent 不得复活"
    );
}
