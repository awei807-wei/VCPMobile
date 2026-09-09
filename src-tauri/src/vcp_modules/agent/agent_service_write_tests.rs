use super::*;
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::Topic;
use sqlx::{Row, SqlitePool};
use std::path::PathBuf;
use tauri::Manager;

fn config(id: &str, name: &str) -> AgentConfig {
    AgentConfig {
        id: id.to_string(),
        name: name.to_string(),
        system_prompt: "system".to_string(),
        mobile_system_prompt: String::new(),
        model: "model".to_string(),
        temperature: 1.0,
        context_token_limit: 100,
        max_output_tokens: 20,
        stream_output: true,
        use_temperature: true,
        avatar_calculated_color: None,
        topics: Vec::new(),
    }
}

fn changed_config() -> AgentConfig {
    AgentConfig {
        id: "agent-a".to_string(),
        name: "new-name".to_string(),
        system_prompt: "new-system".to_string(),
        mobile_system_prompt: "new-mobile-system".to_string(),
        model: "new-model".to_string(),
        temperature: 1.5,
        context_token_limit: 200,
        max_output_tokens: 40,
        stream_output: false,
        use_temperature: false,
        avatar_calculated_color: Some("new-color".to_string()),
        topics: Vec::new(),
    }
}

async fn pool(with_topic_hydration_columns: bool) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
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
            config_hash TEXT NOT NULL,
            content_hash TEXT NOT NULL DEFAULT '',
            updated_at INTEGER NOT NULL,
            deleted_at INTEGER
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
        "INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model,
            temperature, context_token_limit, max_output_tokens,
            stream_output, use_temperature, config_hash, content_hash, updated_at
         ) VALUES ('agent-a', 'old', 'old-system', '', 'old-model', 1, 100, 20, 1, 1,
                   'old-config-hash', 'old-content-hash', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    if with_topic_hydration_columns {
        sqlx::query(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, updated_at,
                locked, unread, unread_count, msg_count, config_hash, content_hash
             ) VALUES ('agent', 'agent-a', 'topic-old', '权威话题', 3, 4, 0, 1, 7, 11,
                       'topic-config-hash', 'topic-content-hash')",
        )
        .execute(&pool)
        .await
        .unwrap();
    } else {
        sqlx::query(
            "INSERT INTO topics (owner_type, owner_id, topic_id, config_hash, content_hash)
             VALUES ('agent', 'agent-a', 'topic-old', 'topic-config-hash', 'topic-content-hash')",
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
    app.manage(AgentConfigState::new());
    app
}

fn old_config() -> AgentConfig {
    AgentConfig {
        id: "agent-a".to_string(),
        name: "old".to_string(),
        system_prompt: "old-system".to_string(),
        mobile_system_prompt: String::new(),
        model: "old-model".to_string(),
        temperature: 1.0,
        context_token_limit: 100,
        max_output_tokens: 20,
        stream_output: true,
        use_temperature: true,
        avatar_calculated_color: None,
        topics: Vec::new(),
    }
}

async fn assert_saved_agent_topic_row(pool: &SqlitePool, before: &sqlx::sqlite::SqliteRow) {
    let after_topic = sqlx::query(
        "SELECT topic_id, title, created_at, updated_at, locked, unread, unread_count,
                msg_count, config_hash, content_hash
         FROM topics WHERE owner_type = 'agent' AND owner_id = 'agent-a'",
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
async fn bubble_failure_rolls_back_agent_config_transaction() {
    let app = test_app(pool(false).await);
    sqlx::query(
        "CREATE TRIGGER fail_agent_bubble
         BEFORE UPDATE OF content_hash ON agents
         BEGIN SELECT RAISE(ABORT, 'injected agent bubble failure'); END",
    )
    .execute(&app.state::<DbState>().pool)
    .await
    .unwrap();

    let before = old_config();
    let error = save_agent_config(app.handle().clone(), app.state(), changed_config())
        .await
        .expect_err("injected bubble failure should abort the write");
    assert!(error.contains("injected agent bubble failure"), "{error}");

    let row = sqlx::query(
        "SELECT name, system_prompt, mobile_system_prompt, model, temperature,
                context_token_limit, max_output_tokens, stream_output, use_temperature,
                config_hash, content_hash
         FROM agents WHERE agent_id = 'agent-a'",
    )
    .fetch_one(&app.state::<DbState>().pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("name"), before.name);
    assert_eq!(row.get::<String, _>("system_prompt"), before.system_prompt);
    assert_eq!(
        row.get::<String, _>("mobile_system_prompt"),
        before.mobile_system_prompt
    );
    assert_eq!(row.get::<String, _>("model"), before.model);
    assert_eq!(row.get::<f64, _>("temperature"), before.temperature);
    assert_eq!(
        row.get::<i64, _>("context_token_limit"),
        i64::from(before.context_token_limit)
    );
    assert_eq!(
        row.get::<i64, _>("max_output_tokens"),
        i64::from(before.max_output_tokens)
    );
    assert_eq!(
        row.get::<i64, _>("stream_output") != 0,
        before.stream_output
    );
    assert_eq!(
        row.get::<i64, _>("use_temperature") != 0,
        before.use_temperature
    );
    assert_eq!(row.get::<String, _>("config_hash"), "old-config-hash");
    assert_eq!(row.get::<String, _>("content_hash"), "old-content-hash");
}

#[tokio::test]
async fn topic_hydration_failure_rolls_back_agent_config_hash() {
    let app = test_app(pool(false).await);
    let error = save_agent_config(app.handle().clone(), app.state(), changed_config())
        .await
        .expect_err("缺少 hydration 列时保存必须在提交前失败");
    assert!(error.contains("title"), "{error}");

    let row = sqlx::query(
        "SELECT name, system_prompt, mobile_system_prompt, model, temperature,
                context_token_limit, max_output_tokens, stream_output, use_temperature,
                config_hash, content_hash
         FROM agents WHERE agent_id = 'agent-a'",
    )
    .fetch_one(&app.state::<DbState>().pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("name"), "old");
    assert_eq!(row.get::<String, _>("system_prompt"), "old-system");
    assert_eq!(row.get::<String, _>("mobile_system_prompt"), "");
    assert_eq!(row.get::<String, _>("model"), "old-model");
    assert_eq!(row.get::<f64, _>("temperature"), 1.0);
    assert_eq!(row.get::<i64, _>("context_token_limit"), 100);
    assert_eq!(row.get::<i64, _>("max_output_tokens"), 20);
    assert_eq!(row.get::<i64, _>("stream_output"), 1);
    assert_eq!(row.get::<i64, _>("use_temperature"), 1);
    assert_eq!(row.get::<String, _>("config_hash"), "old-config-hash");
    assert_eq!(row.get::<String, _>("content_hash"), "old-content-hash");
}

#[tokio::test]
async fn save_agent_config_uses_authoritative_topics_without_writing_input_snapshot() {
    let app = test_app(pool(true).await);
    let before_topic = sqlx::query(
        "SELECT title, created_at, updated_at, locked, unread, unread_count, msg_count,
                config_hash, content_hash
         FROM topics WHERE owner_type = 'agent' AND owner_id = 'agent-a'
           AND topic_id = 'topic-old'",
    )
    .fetch_one(&app.state::<DbState>().pool)
    .await
    .unwrap();
    let mut incoming = config("agent-a", "incoming");
    incoming.topics = vec![Topic {
        id: "evil-topic".to_string(),
        name: "恶意快照".to_string(),
        created_at: 999,
        locked: true,
        unread: true,
        unread_count: 99,
        msg_count: 99,
        owner_id: "group-other".to_string(),
        owner_type: "group".to_string(),
    }];

    assert!(
        save_agent_config(app.handle().clone(), app.state(), incoming)
            .await
            .expect("保存 Agent 应成功")
    );
    assert_saved_agent_topic_row(&app.state::<DbState>().pool, &before_topic).await;
}

#[path = "agent_service_write_update_tests.rs"]
mod update_tests;
