use super::{read_agent_config, read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn save_agent_config(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
    mut agent: AgentConfig,
) -> Result<bool, String> {
    if agent.id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    let agent_id = agent.id.clone();
    let mutex = state.acquire_lock(&agent_id).await;
    let _lock = mutex.lock().await;
    if let Some(cached) = state.caches.get(&agent_id) {
        agent.system_prompt = cached.value().system_prompt.clone();
    } else if let Ok(db_config) =
        read_agent_config_internal(&app_handle, &state, &agent_id, Some(false)).await
    {
        agent.system_prompt = db_config.system_prompt;
    }
    internal_write_agent_config(&app_handle, &state, &agent_id, &agent, false, false).await
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
    let config = read_agent_config(
        app_handle.clone(),
        state.clone(),
        agent_id.clone(),
        Some(true),
    )
    .await?;
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
    internal_write_agent_config(&app_handle, &state, &agent_id, &new_config, false, false).await?;
    Ok(new_config)
}

pub(crate) async fn internal_write_agent_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    new_config: &AgentConfig,
    skip_bubble: bool,
    _from_sync: bool,
) -> Result<bool, String> {
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let final_config = merge_system_prompt(pool, state, agent_id, new_config).await?;
    let dto = AgentSyncDTO::from(&final_config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    upsert_agent_row(&mut tx, agent_id, &final_config, &config_hash, now).await?;
    upsert_agent_topics(&mut tx, agent_id, &new_config.topics, now).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    if !skip_bubble && !new_config.topics.is_empty() {
        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_agent_hash(&mut bubble_tx, agent_id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;
    }
    state.caches.insert(agent_id.to_string(), final_config);
    Ok(true)
}

async fn merge_system_prompt(
    pool: &sqlx::SqlitePool,
    state: &AgentConfigState,
    agent_id: &str,
    config: &AgentConfig,
) -> Result<AgentConfig, String> {
    if !config.system_prompt.is_empty() {
        return Ok(config.clone());
    }
    let mut merged = config.clone();
    if let Some(cached) = state.caches.get(agent_id) {
        merged.system_prompt = cached.value().system_prompt.clone();
    } else if let Some(row) =
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

async fn upsert_agent_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
    config: &AgentConfig,
    config_hash: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
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
            updated_at = excluded.updated_at",
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
    Ok(())
}

async fn upsert_agent_topics(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_id: &str,
    topics: &[crate::vcp_modules::topic_types::Topic],
    now: i64,
) -> Result<(), String> {
    for topic in topics {
        sqlx::query(
            "INSERT INTO topics (
                topic_id, owner_type, owner_id, title, created_at, updated_at, locked, unread
             ) VALUES (?, 'agent', ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
                title = excluded.title, locked = excluded.locked, unread = excluded.unread,
                updated_at = excluded.updated_at",
        )
        .bind(&topic.id)
        .bind(agent_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(now)
        .bind(topic.locked)
        .bind(topic.unread)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let key = TopicKey::new("agent", agent_id, topic.id.clone());
        HashAggregator::bubble_topic_hash_for_key(tx, &key).await?;
    }
    Ok(())
}
