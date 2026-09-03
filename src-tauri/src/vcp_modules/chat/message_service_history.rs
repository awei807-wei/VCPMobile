use super::message_service_support::{load_attachments_for_topic, parse_render_bytes, topic_key};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, RENDERER_SCHEMA_VERSION};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::Row;
use tauri::{AppHandle, Manager};

#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history_internal(
    app_handle: &AppHandle,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    include_content: bool,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let key = topic_key(owner_id, owner_type, topic_id)?;
    let offset = offset.unwrap_or(0);
    let rows = fetch_history_rows(pool, &key, limit, offset).await?;
    let msg_ids = rows
        .iter()
        .map(|row| {
            row.try_get::<String, _>("msg_id")
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut attachment_map =
        load_attachments_for_topic(pool, &key, &msg_ids, include_extracted_text).await?;
    let (agents, user_name, user_avatar_color) = load_history_shell_context(app_handle, pool).await;

    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        let msg_id: String = row.get("msg_id");
        let attachments = attachment_map.remove(&msg_id);
        history.push(build_history_message(
            pool,
            &key,
            &row,
            msg_id,
            include_content,
            attachments,
            &agents,
            &user_name,
            user_avatar_color.as_deref(),
        )?);
    }
    history.reverse();
    Ok(history)
}

async fn fetch_history_rows(
    pool: &sqlx::SqlitePool,
    key: &crate::vcp_modules::topic_types::TopicKey,
    limit: Option<usize>,
    offset: usize,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    let query_string = if limit.is_some() {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) AS name,
                m.agent_id, m.content, m.timestamp, m.updated_at,
                m.is_group_message, m.group_id, m.finish_reason,
                r.render_content, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
           AND m.deleted_at IS NULL
         ORDER BY m.timestamp DESC, m.rowid DESC
         LIMIT ? OFFSET ?"
    } else {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) AS name,
                m.agent_id, m.content, m.timestamp, m.updated_at,
                m.is_group_message, m.group_id, m.finish_reason,
                r.render_content, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         LEFT JOIN agents a ON m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
           AND m.deleted_at IS NULL
         ORDER BY m.timestamp DESC, m.rowid DESC"
    };
    let mut query = sqlx::query(query_string)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    if let Some(limit) = limit {
        query = query.bind(limit as i64).bind(offset as i64);
    }
    query
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

async fn load_history_shell_context(
    app_handle: &AppHandle,
    pool: &sqlx::SqlitePool,
) -> (
    Vec<crate::vcp_modules::agent_types::AgentConfig>,
    String,
    Option<String>,
) {
    let agents = load_history_agents(pool).await;
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), app_handle.state())
            .await
            .ok();
    let user_name = settings
        .map(|settings| settings.user_name)
        .unwrap_or_else(|| "User".to_string());
    let user_avatar_color: Option<String> = sqlx::query_scalar(
        "SELECT dominant_color FROM avatars
         WHERE owner_type = 'user' AND owner_id = 'user_avatar'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    (agents, user_name, user_avatar_color)
}

async fn load_history_agents(
    pool: &sqlx::SqlitePool,
) -> Vec<crate::vcp_modules::agent_types::AgentConfig> {
    match sqlx::query(
        "SELECT a.agent_id, a.name, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
         WHERE a.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows
            .into_iter()
            .map(|row| crate::vcp_modules::agent_types::AgentConfig {
                id: row.get("agent_id"),
                name: row.get("name"),
                avatar_calculated_color: row.get("dominant_color"),
                system_prompt: String::new(),
                mobile_system_prompt: String::new(),
                model: String::new(),
                temperature: 0.0,
                context_token_limit: 0,
                max_output_tokens: 0,
                stream_output: false,
                use_temperature: false,
                topics: vec![],
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_history_message(
    pool: &sqlx::SqlitePool,
    key: &crate::vcp_modules::topic_types::TopicKey,
    row: &sqlx::sqlite::SqliteRow,
    msg_id: String,
    include_content: bool,
    attachments: Option<Vec<crate::vcp_modules::chat_manager::Attachment>>,
    agents: &[crate::vcp_modules::agent_types::AgentConfig],
    user_name: &str,
    user_avatar_color: Option<&str>,
) -> Result<ChatMessage, String> {
    let decoded_content = decode_message_content(row, "content")?;
    let (blocks, content) =
        render_history_content(pool, key, row, &msg_id, decoded_content, include_content);
    let content_hash: String = row.get("content_hash");
    let timestamp: i64 = row.get("timestamp");
    let updated_at: i64 = row.get("updated_at");
    let mut message = ChatMessage {
        id: msg_id,
        role: row.get("role"),
        name: row.get("name"),
        content,
        timestamp: u64::try_from(timestamp)
            .map_err(|_| "message timestamp is negative".to_string())?,
        updated_at: Some(
            u64::try_from(updated_at).map_err(|_| "message updated_at is negative".to_string())?,
        ),
        is_thinking: Some(false),
        agent_id: row.get("agent_id"),
        group_id: row.get("group_id"),
        topic_id: Some(key.topic_id.clone()),
        is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
        finish_reason: row.get("finish_reason"),
        attachments,
        blocks,
        shell: None,
        content_hash: (!content_hash.is_empty()).then_some(content_hash),
    };
    message.shell = Some(crate::vcp_modules::pre_renderer::precompute_shell(
        &message,
        agents,
        user_name,
        user_avatar_color,
    ));
    Ok(message)
}

fn render_history_content(
    pool: &sqlx::SqlitePool,
    key: &crate::vcp_modules::topic_types::TopicKey,
    row: &sqlx::sqlite::SqliteRow,
    msg_id: &str,
    decoded_content: String,
    include_content: bool,
) -> (Option<serde_json::Value>, String) {
    if let Some(render_content) = row.get::<Option<Vec<u8>>, _>("render_content") {
        return (
            parse_render_bytes(Some(render_content)),
            content_if_requested(decoded_content, include_content),
        );
    }
    if decoded_content.is_empty() {
        return (None, String::new());
    }
    let compiled = MessageRenderCompiler::compile(&decoded_content);
    if let Ok(serialized) = MessageRenderCompiler::serialize(&compiled) {
        schedule_render_cache(pool, key, msg_id, serialized);
    }
    (
        serde_json::to_value(&compiled).ok(),
        content_if_requested(decoded_content, include_content),
    )
}

fn content_if_requested(content: String, include_content: bool) -> String {
    if include_content {
        content
    } else {
        String::new()
    }
}

fn schedule_render_cache(
    pool: &sqlx::SqlitePool,
    key: &crate::vcp_modules::topic_types::TopicKey,
    msg_id: &str,
    serialized: Vec<u8>,
) {
    let pool = pool.clone();
    let owner_type = key.owner_type.clone();
    let owner_id = key.owner_id.clone();
    let topic_id = key.topic_id.clone();
    let message_id = msg_id.to_string();
    tokio::spawn(async move {
        let now = chrono::Utc::now().timestamp_millis();
        let _ = sqlx::query(
            "INSERT INTO render_cache (
                owner_type, owner_id, topic_id, msg_id, render_content,
                content_hash, renderer_schema_version, updated_at
             )
             SELECT ?, ?, ?, ?, ?, m.content_hash, ?, ?
             FROM messages m
             WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
               AND m.msg_id = ? AND m.deleted_at IS NULL
             ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                render_content = excluded.render_content,
                content_hash = excluded.content_hash,
                renderer_schema_version = excluded.renderer_schema_version,
                updated_at = excluded.updated_at",
        )
        .bind(&owner_type)
        .bind(&owner_id)
        .bind(&topic_id)
        .bind(&message_id)
        .bind(&serialized)
        .bind(RENDERER_SCHEMA_VERSION)
        .bind(now)
        .bind(&owner_type)
        .bind(&owner_id)
        .bind(&topic_id)
        .bind(&message_id)
        .execute(&pool)
        .await;
    });
}
