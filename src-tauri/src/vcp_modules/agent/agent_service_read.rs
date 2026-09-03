use super::{create_default_config, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime, State};

/// 前端专属数据加载指令：清空返回对象中的 system_prompt。
#[tauri::command]
pub async fn read_agent_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, AgentConfigState>,
    agent_id: String,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    let mut config =
        read_agent_config_internal(&app_handle, &state, &agent_id, allow_default).await?;
    config.system_prompt.clear();
    Ok(config)
}

pub async fn read_agent_config_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &AgentConfigState,
    agent_id: &str,
    allow_default: Option<bool>,
) -> Result<AgentConfig, String> {
    if let Some(cached) = state.caches.get(agent_id) {
        return Ok(cached.value().clone());
    }

    let db_state = app_handle.state::<DbState>();
    let agent_row = sqlx::query(
        "SELECT a.name, a.system_prompt, a.mobile_system_prompt, a.model, a.temperature,
                a.context_token_limit, a.max_output_tokens, a.stream_output,
                a.use_temperature, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
         WHERE a.agent_id = ? AND a.deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    let Some(row) = agent_row else {
        return if allow_default.unwrap_or(false) {
            Ok(create_default_config(agent_id))
        } else {
            Err(format!("Agent {agent_id} not found"))
        };
    };

    let topics = load_agent_topics(&db_state.pool, agent_id).await?;

    let config = AgentConfig {
        id: agent_id.to_string(),
        name: row.get("name"),
        system_prompt: row.get("system_prompt"),
        mobile_system_prompt: row.get("mobile_system_prompt"),
        model: row.get("model"),
        temperature: row.get("temperature"),
        context_token_limit: row.get("context_token_limit"),
        max_output_tokens: row.get("max_output_tokens"),
        stream_output: row.get::<i32, _>("stream_output") != 0,
        use_temperature: row.get::<i32, _>("use_temperature") != 0,
        avatar_calculated_color: row.get("dominant_color"),
        topics,
    };
    state.caches.insert(agent_id.to_string(), config.clone());
    Ok(config)
}

async fn load_agent_topics(pool: &sqlx::SqlitePool, agent_id: &str) -> Result<Vec<Topic>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC",
    )
    .bind(agent_id)
    .fetch_all(pool)
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

#[tauri::command]
pub async fn get_agents(
    app_handle: AppHandle,
    state: State<'_, AgentConfigState>,
) -> Result<Vec<AgentConfig>, String> {
    let start_total = std::time::Instant::now();
    let agent_rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.system_prompt, a.mobile_system_prompt, a.model,
                a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output,
                a.use_temperature, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
         WHERE a.deleted_at IS NULL",
    )
    .fetch_all(&app_handle.state::<DbState>().pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut agents = Vec::with_capacity(agent_rows.len());
    for row in agent_rows {
        let agent_id: String = row.get("agent_id");
        let config = AgentConfig {
            id: agent_id.clone(),
            name: row.get("name"),
            system_prompt: row.get("system_prompt"),
            mobile_system_prompt: row.get("mobile_system_prompt"),
            model: row.get("model"),
            temperature: row.get("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i32, _>("stream_output") != 0,
            use_temperature: row.get::<i32, _>("use_temperature") != 0,
            avatar_calculated_color: row.get("dominant_color"),
            topics: vec![],
        };
        state.caches.insert(agent_id, config.clone());
        agents.push(config);
    }
    log::info!(
        "[Profile] get_agents finished. Total: {}ms | Agents: {}",
        start_total.elapsed().as_millis(),
        agents.len()
    );
    Ok(agents)
}
