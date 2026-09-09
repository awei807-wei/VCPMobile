use serde::Serialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::vcp_modules::message_repository::ContentCompressor;

use super::TOOL_NAME;

#[derive(Debug, Clone)]
pub(super) struct AgentInfo {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TopicRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) created_at: i64,
    pub(super) locked: bool,
    pub(super) unread: bool,
    pub(super) unread_count: i32,
    pub(super) msg_count: i32,
    pub(super) owner_id: String,
    pub(super) owner_type: String,
}

pub(super) async fn load_topics(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
    locked: Option<bool>,
    created_after: Option<i64>,
) -> Result<Vec<TopicRow>, String> {
    let mut sql = "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL"
        .to_string();
    if locked.is_some() {
        sql.push_str(" AND locked = ?");
    }
    if created_after.is_some() {
        sql.push_str(" AND created_at > ?");
    }
    sql.push_str(" ORDER BY updated_at DESC, created_at DESC");

    let mut query = sqlx::query(&sql).bind(agent_id);
    if let Some(locked) = locked {
        query = query.bind(if locked { 1 } else { 0 });
    }
    if let Some(created_after) = created_after {
        query = query.bind(created_after);
    }

    let rows = query.fetch_all(pool).await.map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| TopicRow {
            id: row.get("topic_id"),
            name: row.get("title"),
            created_at: row.get("created_at"),
            locked: row.get::<i32, _>("locked") != 0,
            unread: row.get::<i32, _>("unread") != 0,
            msg_count: row.get("msg_count"),
            unread_count: row.try_get("unread_count").unwrap_or(0),
            owner_id: agent_id.to_string(),
            owner_type: "agent".to_string(),
        })
        .collect())
}

pub(super) async fn find_topic(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: &str,
    topic_id: &str,
) -> Result<TopicRow, String> {
    let row = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         LIMIT 1",
    )
    .bind(agent_id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("话题 {} 不存在。", topic_id))?;

    Ok(TopicRow {
        id: row.get("topic_id"),
        name: row.get("title"),
        created_at: row.get("created_at"),
        locked: row.get::<i32, _>("locked") != 0,
        unread: row.get::<i32, _>("unread") != 0,
        unread_count: row.get("unread_count"),
        msg_count: row.get("msg_count"),
        owner_id: agent_id.to_string(),
        owner_type: "agent".to_string(),
    })
}

pub(super) async fn load_messages(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    topic_id: &str,
) -> Result<Vec<Value>, String> {
    let rows = sqlx::query(
        "SELECT msg_id, role, name, content, timestamp, agent_id, finish_reason
         FROM messages
         WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC",
    )
    .bind(owner_id)
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    rows.into_iter()
        .map(|row| {
            let msg_id = row.get::<String, _>("msg_id");
            let content_bytes: Vec<u8> = row.get("content");
            let (content, content_corrupted) = match ContentCompressor::decompress(&content_bytes) {
                Ok(content) => (content, false),
                Err(error) => {
                    log::warn!(
                        "[{}] Failed to decompress message {} in topic {}: {}",
                        TOOL_NAME,
                        msg_id,
                        topic_id,
                        error
                    );
                    (format!("[消息内容解压失败: {}]", error), true)
                }
            };
            Ok(json!({
                "role": row.get::<String, _>("role"),
                "name": row.get::<Option<String>, _>("name"),
                "content": content,
                "timestamp": row.get::<i64, _>("timestamp"),
                "id": msg_id,
                "agentId": row.get::<Option<String>, _>("agent_id"),
                "finishReason": row.get::<Option<String>, _>("finish_reason"),
                "contentCorrupted": content_corrupted
            }))
        })
        .collect()
}

pub(super) async fn first_message_name(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    topic_id: &str,
) -> Result<Option<String>, String> {
    let name = sqlx::query_scalar::<_, Option<String>>(
        "SELECT name FROM messages
         WHERE owner_type = 'agent' AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC LIMIT 1",
    )
    .bind(owner_id)
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(name.flatten())
}

pub(super) async fn find_agent_info(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    maid_name: &str,
) -> Result<AgentInfo, String> {
    let row = sqlx::query(
        "SELECT agent_id, name
         FROM agents
         WHERE deleted_at IS NULL AND name = ?
         ORDER BY updated_at DESC
         LIMIT 1",
    )
    .bind(maid_name)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    let row = match row {
        Some(row) => row,
        None => {
            let pattern = format!("%{}%", escape_like_pattern(maid_name));
            sqlx::query(
                "SELECT agent_id, name
                 FROM agents
                 WHERE deleted_at IS NULL AND name LIKE ? ESCAPE '\\'
                 ORDER BY updated_at DESC
                 LIMIT 1",
            )
            .bind(pattern)
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("未找到名为 \"{}\" 的 Agent。", maid_name))?
        }
    };

    Ok(AgentInfo {
        id: row.get("agent_id"),
        name: row.get("name"),
    })
}

pub(super) fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub(super) fn get_string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn get_topic_id_arg(args: &Value) -> Option<String> {
    ["topic_id", "topicId", "TopicId"]
        .iter()
        .find_map(|key| get_string_arg(args, key))
}

pub(super) fn get_bool_arg(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => value.trim().parse::<bool>().ok(),
        _ => None,
    }
}

pub(super) fn get_i64_arg(args: &Value, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}
