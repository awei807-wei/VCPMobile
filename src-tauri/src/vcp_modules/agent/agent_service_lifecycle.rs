use super::AgentConfigState;
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType};
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use tauri::{AppHandle, Manager, Runtime, State};

/// 删除 Agent，并按完整 owner 身份级联其话题、消息和生成注册表。
#[tauri::command]
pub async fn delete_agent<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    agent_id: String,
) -> Result<bool, String> {
    let owner_lock = state.acquire_lock(&agent_id).await;
    let _owner_guard = owner_lock.lock().await;
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    delete_agent_data(pool, &agent_id, now).await?;
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Owner {
                owner_type: OwnerType::Agent,
                owner_id: agent_id,
            },
            deleted_at: now,
        });
    }
    Ok(true)
}

pub(crate) async fn delete_agent_data(
    pool: &SqlitePool,
    agent_id: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let result = sqlx::query(
        "UPDATE agents SET deleted_at = ?
         WHERE agent_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(agent_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if result.rows_affected() != 1 {
        return Err(format!("Agent {agent_id} 不存在或已删除"));
    }
    mark_agent_topics_deleted(&mut tx, agent_id, now).await?;
    mark_agent_messages_deleted(&mut tx, agent_id, now).await?;
    clear_agent_unread_receipts(&mut tx, agent_id).await?;
    clear_agent_attachment_relations(&mut tx, agent_id).await?;
    clear_agent_generations(&mut tx, agent_id).await?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn clear_agent_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind(agent_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn mark_agent_topics_deleted(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(agent_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn mark_agent_messages_deleted(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'agent' AND owner_id = ?
           AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(agent_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_agent_generations(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind(agent_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_agent_attachment_relations(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = ?",
    )
    .bind(agent_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 创建 Agent 及其初始话题，保证所有写入处携带 agent owner 身份。
#[tauri::command]
pub async fn create_agent<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    name: String,
    initial_config: Option<serde_json::Value>,
) -> Result<AgentConfig, String> {
    let timestamp = crate::vcp_modules::infra::utils::now_millis();
    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let agent_id = format!("{base_id}_{timestamp}");
    let config = build_new_agent_config(&agent_id, &name, timestamp, initial_config)?;
    let owner_lock = state.acquire_lock(&agent_id).await;
    let _owner_guard = owner_lock.lock().await;
    let pool = &app_handle.state::<DbState>().pool;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let dto = AgentSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    insert_agent_row(&mut tx, &agent_id, &config, &config_hash, timestamp).await?;
    insert_agent_topics(&mut tx, &agent_id, &config.topics, timestamp).await?;
    HashAggregator::bubble_agent_hash(&mut tx, &agent_id).await?;
    let mut persisted_config = config;
    persisted_config.topics = load_agent_topics(&mut tx, &agent_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(persisted_config)
}

fn build_new_agent_config(
    agent_id: &str,
    name: &str,
    timestamp: i64,
    initial_config: Option<serde_json::Value>,
) -> Result<AgentConfig, String> {
    if let Some(init) = initial_config {
        let mut config: AgentConfig = serde_json::from_value(init).map_err(|e| e.to_string())?;
        config.id = agent_id.to_string();
        config.name = name.to_string();
        return Ok(config);
    }
    Ok(AgentConfig {
        id: agent_id.to_string(),
        name: name.to_string(),
        system_prompt: format!("你是 {name}。"),
        mobile_system_prompt: format!("你是 {name}。"),
        model: "gemini-2.5-flash".to_string(),
        temperature: 0.7,
        context_token_limit: 1_000_000,
        max_output_tokens: 60_000,
        stream_output: true,
        use_temperature: false,
        avatar_calculated_color: None,
        topics: vec![Topic {
            id: format!("topic_{timestamp}"),
            name: "主要对话".to_string(),
            created_at: timestamp,
            locked: true,
            unread: false,
            unread_count: 0,
            msg_count: 0,
            owner_id: agent_id.to_string(),
            owner_type: "agent".to_string(),
        }],
    })
}

async fn insert_agent_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
    config: &AgentConfig,
    config_hash: &str,
    timestamp: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model, temperature,
            context_token_limit, max_output_tokens, stream_output, use_temperature,
            config_hash, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(agent_id)
    .bind(&config.name)
    .bind(&config.system_prompt)
    .bind(&config.mobile_system_prompt)
    .bind(&config.model)
    .bind(config.temperature)
    .bind(config.context_token_limit)
    .bind(config.max_output_tokens)
    .bind(i32::from(config.stream_output))
    .bind(i32::from(config.use_temperature))
    .bind(config_hash)
    .bind(timestamp)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn insert_agent_topics(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
    topics: &[Topic],
    timestamp: i64,
) -> Result<(), String> {
    for topic in topics {
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at)
             VALUES (?, 'agent', ?, ?, ?, ?)",
        )
        .bind(&topic.id)
        .bind(agent_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(timestamp)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let key = TopicKey::new("agent", agent_id, topic.id.clone());
        HashAggregator::bubble_topic_hash_for_key(tx, &key).await?;
    }
    Ok(())
}

async fn load_agent_topics(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
) -> Result<Vec<Topic>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC",
    )
    .bind(agent_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let key = TopicKey::new("agent", agent_id, row.get::<String, _>("topic_id"));
            Topic {
                id: key.topic_id,
                name: row.get("title"),
                created_at: row.get("created_at"),
                locked: row.get::<i32, _>("locked") != 0,
                unread: row.get::<i32, _>("unread") != 0,
                unread_count: row.get("unread_count"),
                msg_count: row.get("msg_count"),
                owner_id: key.owner_id,
                owner_type: key.owner_type,
            }
        })
        .collect())
}
