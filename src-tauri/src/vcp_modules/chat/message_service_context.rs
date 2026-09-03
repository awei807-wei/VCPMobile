use super::message_service_support::{load_attachments_for_topic, resolve_unique_topic_key};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::Row;
use tauri::{AppHandle, Manager};

/// Legacy context-history facade. It is safe only while the topic id is unique.
pub async fn load_chat_text_history_for_context(
    app_handle: &AppHandle,
    topic_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let key = resolve_unique_topic_key(&db_state.pool, topic_id).await?;
    load_chat_text_history_for_topic(app_handle, &key, limit, offset, include_extracted_text).await
}

/// Owner-aware context history loader. Render cache and UI shells are skipped,
/// while compressed message content and attachment extraction are preserved.
pub async fn load_chat_text_history_for_topic(
    app_handle: &AppHandle,
    key: &TopicKey,
    limit: Option<usize>,
    offset: Option<usize>,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    if !key.is_valid() {
        return Err("invalid topic identity".to_string());
    }
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = &db_state.pool;
    let offset = offset.unwrap_or(0);
    let rows = fetch_context_rows(pool, key, limit, offset).await?;
    let msg_ids = rows
        .iter()
        .map(|row| {
            row.try_get::<String, _>("msg_id")
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut attachment_map =
        load_attachments_for_topic(pool, key, &msg_ids, include_extracted_text).await?;
    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        let msg_id: String = row.get("msg_id");
        history.push(decode_context_message(
            &row,
            key,
            attachment_map.remove(&msg_id),
        )?);
    }
    history.reverse();
    Ok(history)
}

async fn fetch_context_rows(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    limit: Option<usize>,
    offset: usize,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    let query_string = if limit.is_some() {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) AS name,
                m.agent_id, m.content, m.timestamp, m.updated_at,
                m.is_group_message, m.group_id, m.finish_reason, m.content_hash
         FROM messages m
         LEFT JOIN agents a ON m.agent_id = a.agent_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
           AND m.deleted_at IS NULL
         ORDER BY m.timestamp DESC, m.rowid DESC
         LIMIT ? OFFSET ?"
    } else {
        "SELECT m.msg_id, m.role, COALESCE(m.name, a.name) AS name,
                m.agent_id, m.content, m.timestamp, m.updated_at,
                m.is_group_message, m.group_id, m.finish_reason, m.content_hash
         FROM messages m
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

fn decode_context_message(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
    attachments: Option<Vec<crate::vcp_modules::chat_manager::Attachment>>,
) -> Result<ChatMessage, String> {
    let msg_id: String = row.get("msg_id");
    let content_hash: String = row.get("content_hash");
    let timestamp: i64 = row.get("timestamp");
    let updated_at: i64 = row.get("updated_at");
    Ok(ChatMessage {
        id: msg_id,
        role: row.get("role"),
        name: row.get("name"),
        content: decode_message_content(row, "content")?,
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
        blocks: None,
        shell: None,
        content_hash: (!content_hash.is_empty()).then_some(content_hash),
    })
}
