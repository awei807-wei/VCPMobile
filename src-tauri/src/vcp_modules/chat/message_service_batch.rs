use super::message_service_support::{
    load_attachments_for_message_keys, parse_render_bytes, resolve_unique_topic_key,
};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::Row;
use std::collections::HashMap;

/// Legacy topic-only batch facade. It resolves every topic before querying and
/// rejects duplicate ids across owners instead of guessing a namespace.
pub async fn load_multi_topic_messages(
    pool: &sqlx::SqlitePool,
    topic_ids: &[String],
) -> Result<HashMap<String, Vec<ChatMessage>>, String> {
    let mut keys = Vec::with_capacity(topic_ids.len());
    for topic_id in topic_ids {
        keys.push(resolve_unique_topic_key(pool, topic_id).await?);
    }
    let keyed_messages = load_multi_topic_messages_for_keys(pool, &keys).await?;
    let mut result = topic_ids
        .iter()
        .map(|topic_id| (topic_id.clone(), Vec::new()))
        .collect::<HashMap<_, _>>();
    for (key, messages) in keyed_messages {
        result.entry(key.topic_id).or_default().extend(messages);
    }
    Ok(result)
}

/// Owner-aware batch loader used by Wire 1.4 callers.
pub async fn load_multi_topic_messages_for_keys(
    pool: &sqlx::SqlitePool,
    topic_keys: &[TopicKey],
) -> Result<HashMap<TopicKey, Vec<ChatMessage>>, String> {
    let mut result = empty_topic_message_map(topic_keys);
    if topic_keys.is_empty() {
        return Ok(result);
    }
    let rows = fetch_topic_message_rows(pool, topic_keys).await?;
    let mut message_keys = Vec::with_capacity(rows.len());
    for row in rows {
        let (key, message) = decode_topic_message(&row)?;
        let message_key = MessageKey::new(key.clone(), message.id.clone());
        result.entry(key).or_default().push(message);
        message_keys.push(message_key);
    }
    let attachments = load_attachments_for_message_keys(pool, &message_keys).await?;
    apply_batch_attachments(&mut result, &attachments);
    Ok(result)
}

fn empty_topic_message_map(topic_keys: &[TopicKey]) -> HashMap<TopicKey, Vec<ChatMessage>> {
    topic_keys
        .iter()
        .cloned()
        .map(|key| (key, Vec::new()))
        .collect()
}

async fn fetch_topic_message_rows(
    pool: &sqlx::SqlitePool,
    topic_keys: &[TopicKey],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    let tuples = topic_keys
        .iter()
        .map(|_| "(?, ?, ?)")
        .collect::<Vec<_>>()
        .join(",");
    let query_string = format!(
        "SELECT m.msg_id, m.role, m.name, m.agent_id, m.content, m.timestamp,
                m.updated_at, m.is_group_message, m.group_id, m.finish_reason,
                r.render_content, m.owner_type, m.owner_id, m.topic_id, m.content_hash
         FROM messages m
         LEFT JOIN render_cache r
           ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
          AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
         WHERE (m.owner_type, m.owner_id, m.topic_id) IN ({tuples})
           AND m.deleted_at IS NULL
         ORDER BY m.owner_type, m.owner_id, m.topic_id,
                  m.timestamp ASC, m.msg_id ASC"
    );
    let mut query = sqlx::query(&query_string);
    for key in topic_keys {
        query = query
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
    }
    query
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

fn decode_topic_message(row: &sqlx::sqlite::SqliteRow) -> Result<(TopicKey, ChatMessage), String> {
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| error.to_string())?;
    let owner_id: String = row.try_get("owner_id").map_err(|error| error.to_string())?;
    let topic_id: String = row.try_get("topic_id").map_err(|error| error.to_string())?;
    let key = TopicKey::new(owner_type, owner_id, topic_id.clone());
    let message_id: String = row.try_get("msg_id").map_err(|error| error.to_string())?;
    let timestamp: i64 = row
        .try_get("timestamp")
        .map_err(|error| error.to_string())?;
    let updated_at: i64 = row
        .try_get("updated_at")
        .map_err(|error| error.to_string())?;
    let content_hash: String = row
        .try_get("content_hash")
        .map_err(|error| error.to_string())?;
    let message = ChatMessage {
        id: message_id,
        role: row.try_get("role").map_err(|error| error.to_string())?,
        name: row.try_get("name").map_err(|error| error.to_string())?,
        content: decode_message_content(row, "content")?,
        timestamp: u64::try_from(timestamp)
            .map_err(|_| "message timestamp is negative".to_string())?,
        updated_at: Some(
            u64::try_from(updated_at).map_err(|_| "message updated_at is negative".to_string())?,
        ),
        is_thinking: Some(false),
        agent_id: row.try_get("agent_id").map_err(|error| error.to_string())?,
        group_id: row.try_get("group_id").map_err(|error| error.to_string())?,
        topic_id: Some(topic_id),
        is_group_message: Some(
            row.try_get::<i64, _>("is_group_message")
                .map_err(|error| error.to_string())?
                != 0,
        ),
        finish_reason: row
            .try_get("finish_reason")
            .map_err(|error| error.to_string())?,
        attachments: None,
        blocks: parse_render_bytes(
            row.try_get("render_content")
                .map_err(|error| error.to_string())?,
        ),
        shell: None,
        content_hash: (!content_hash.is_empty()).then_some(content_hash),
    };
    Ok((key, message))
}

fn apply_batch_attachments(
    messages: &mut HashMap<TopicKey, Vec<ChatMessage>>,
    attachments: &HashMap<MessageKey, Vec<crate::vcp_modules::chat_manager::Attachment>>,
) {
    for (key, values) in messages {
        for message in values {
            let message_key = MessageKey::new(key.clone(), message.id.clone());
            if let Some(attachments) = attachments.get(&message_key) {
                message.attachments = Some(attachments.clone());
            }
        }
    }
}
