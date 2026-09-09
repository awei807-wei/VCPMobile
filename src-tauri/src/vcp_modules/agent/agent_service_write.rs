use super::{read_agent_config_locked, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::{Row, Sqlite, Transaction};
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn save_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    mut agent: AgentConfig,
) -> Result<bool, String> {
    if agent.id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    let agent_id = agent.id.clone();
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;
    if let Ok(db_config) = read_agent_config_locked(&app_handle, &agent_id, Some(false)).await {
        agent.system_prompt = db_config.system_prompt;
    }
    internal_write_agent_config(&app_handle, &agent_id, &agent).await?;
    Ok(true)
}

#[tauri::command]
pub async fn update_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    updates: serde_json::Value,
) -> Result<AgentConfig, String> {
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;
    let mut config = read_agent_config_locked(&app_handle, &agent_id, Some(true)).await?;
    config.system_prompt.clear();
    let mut config_value = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    if let (Some(target), Some(source)) = (config_value.as_object_mut(), updates.as_object()) {
        target.extend(
            source
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let new_config: AgentConfig =
        serde_json::from_value(config_value).map_err(|e| e.to_string())?;
    internal_write_agent_config(&app_handle, &agent_id, &new_config).await
}

pub(crate) async fn internal_write_agent_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    agent_id: &str,
    new_config: &AgentConfig,
) -> Result<AgentConfig, String> {
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let final_config = merge_system_prompt(pool, agent_id, new_config).await?;
    let dto = AgentSyncDTO::from(&final_config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    write_agent_config_in_transaction(&mut tx, agent_id, &final_config, &config_hash, now).await?;
    // The owner hash is part of the configuration write's atomic unit. If
    // bubbling fails, dropping this transaction rolls back both the config
    // row and every member/topic-derived hash update.
    HashAggregator::bubble_agent_hash(&mut tx, agent_id).await?;
    let mut persisted_config = final_config;
    persisted_config.topics = load_agent_topics(&mut tx, agent_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(persisted_config)
}

async fn write_agent_config_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    config: &AgentConfig,
    config_hash: &str,
    now: i64,
) -> Result<(), String> {
    ensure_agent_writeable(tx, agent_id).await?;
    upsert_agent_row(tx, agent_id, config, config_hash, now).await
}

async fn merge_system_prompt(
    pool: &sqlx::SqlitePool,
    agent_id: &str,
    config: &AgentConfig,
) -> Result<AgentConfig, String> {
    if !config.system_prompt.is_empty() {
        return Ok(config.clone());
    }
    let mut merged = config.clone();
    if let Some(row) =
        sqlx::query("SELECT system_prompt FROM agents WHERE agent_id = ? AND deleted_at IS NULL")
            .bind(agent_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?
    {
        merged.system_prompt = row.get("system_prompt");
    }
    Ok(merged)
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

async fn upsert_agent_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
    config: &AgentConfig,
    config_hash: &str,
    now: i64,
) -> Result<(), String> {
    let result = sqlx::query(
        "INSERT INTO agents (
            agent_id, name, system_prompt, mobile_system_prompt, model, temperature,
            context_token_limit, max_output_tokens, stream_output, use_temperature,
            config_hash, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(agent_id) DO UPDATE SET
            name = excluded.name, system_prompt = excluded.system_prompt,
            mobile_system_prompt = excluded.mobile_system_prompt, model = excluded.model,
            temperature = excluded.temperature, context_token_limit = excluded.context_token_limit,
            max_output_tokens = excluded.max_output_tokens, stream_output = excluded.stream_output,
            use_temperature = excluded.use_temperature, config_hash = excluded.config_hash,
            updated_at = excluded.updated_at
         WHERE agents.deleted_at IS NULL",
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
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    if result.rows_affected() != 1 {
        return Err(format!("Agent {agent_id} 已删除或写入影响行数异常"));
    }
    Ok(())
}

async fn ensure_agent_writeable(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
) -> Result<(), String> {
    let deleted_at: Option<Option<i64>> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
    if deleted_at.flatten().is_some() {
        return Err(format!("Agent {agent_id} 已删除"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_service_write_tests.rs"]
mod tests;
