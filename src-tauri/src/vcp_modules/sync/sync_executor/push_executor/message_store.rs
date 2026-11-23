use super::message_attachments::load_message_attachments;
use super::types::{
    MessagePushSelection, SerializedTopicMessages, MAX_MESSAGES_PER_TOPIC, MESSAGE_PAGE_SIZE,
};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::sync::sync_error::{encode_local_sync_error, SyncErrorStage};
use crate::vcp_modules::sync::sync_types::{MessageDeleteDecision, MAX_SAFE_TIMESTAMP};
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};
use std::io::Write;
pub(super) async fn load_outbound_message_page(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    cursor: Option<(i64, &str)>,
) -> Result<Vec<MessageSyncDTO>, String> {
    let mut query = if cursor.is_some() {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp,
                    is_group_message, group_id, finish_reason, content_hash, updated_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    } else {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp,
                    is_group_message, group_id, finish_reason, content_hash, updated_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    };
    query = query
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    if let Some((timestamp, message_id)) = cursor {
        query = query.bind(timestamp).bind(timestamp).bind(message_id);
    }
    let rows = query
        .bind(MESSAGE_PAGE_SIZE as i64)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Message page query failed for {}: {error}", key.topic_id))?;

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        messages.push(decode_message_row(&row, key)?);
    }
    load_message_attachments(tx, key, &mut messages).await
}

fn decode_message_row(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
) -> Result<MessageSyncDTO, String> {
    let (message_id, role) = decode_message_identity(row, key)?;
    if message_id.is_empty() || role.is_empty() {
        return Err(format!(
            "Outbound message {}/{}/{} requires non-empty id and role",
            key.owner_id, key.topic_id, message_id
        ));
    }
    let (timestamp, updated_at, content) = decode_message_payload(row, key, &message_id)?;
    let content_hash: String = row.try_get("content_hash").map_err(|error| {
        format!(
            "Message content hash decode failed for {}/{}: {error}",
            key.topic_id, message_id
        )
    })?;
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&content_hash)
        || content_hash.to_ascii_lowercase() != content_hash
    {
        return Err(format!(
            "Outbound message {}/{}/{} requires a lowercase 64-character contentHash",
            key.owner_id, key.topic_id, message_id
        ));
    }
    let is_group = key.owner_type == "group";
    Ok(MessageSyncDTO {
        id: message_id.clone(),
        role,
        name: row.try_get("name").map_err(|error| {
            format!(
                "Message name decode failed for {}/{}: {error}",
                key.topic_id, message_id
            )
        })?,
        content,
        timestamp: timestamp as u64,
        updated_at: updated_at as u64,
        agent_id: row.try_get("agent_id").map_err(|error| {
            format!(
                "Message agent decode failed for {}/{}: {error}",
                key.topic_id, message_id
            )
        })?,
        group_id: is_group.then(|| key.owner_id.clone()),
        topic_id: Some(key.topic_id.clone()),
        is_group_message: Some(is_group),
        finish_reason: row.try_get("finish_reason").map_err(|error| {
            format!(
                "Message finish reason decode failed for {}/{}: {error}",
                key.topic_id, message_id
            )
        })?,
        attachments: None,
        content_hash,
    })
}

fn decode_message_payload(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
    message_id: &str,
) -> Result<(i64, i64, String), String> {
    let timestamp = decode_non_negative_i64(row, "timestamp", message_id, key)?;
    let updated_at = decode_non_negative_i64(row, "updated_at", message_id, key)?;
    let _is_group_message: i64 = row.try_get("is_group_message").map_err(|error| {
        format!(
            "Message group flag decode failed for {}/{}: {error}",
            key.topic_id, message_id
        )
    })?;
    let content = decode_message_content(row, "content").map_err(|error| {
        format!(
            "Message content decode failed for {}/{}: {error}",
            key.topic_id, message_id
        )
    })?;
    Ok((timestamp, updated_at, content))
}
fn decode_message_identity(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
) -> Result<(String, String), String> {
    let message_id: String = row
        .try_get("msg_id")
        .map_err(|error| format!("Message id decode failed for {}: {error}", key.topic_id))?;
    let role: String = row
        .try_get("role")
        .map_err(|error| format!("Message role decode failed for {}: {error}", key.topic_id))?;
    Ok((message_id, role))
}

fn decode_non_negative_i64(
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    message_id: &str,
    key: &TopicKey,
) -> Result<i64, String> {
    let value: i64 = row.try_get(field).map_err(|error| {
        format!(
            "Message {field} decode failed for {}/{}: {error}",
            key.topic_id, message_id
        )
    })?;
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&value) {
        return Err(format!(
            "Message {}/{}/{} has an invalid {field}; expected a non-negative safe integer",
            key.owner_id, key.topic_id, message_id,
        ));
    }
    Ok(value)
}

pub(super) async fn serialize_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<SerializedTopicMessages, String> {
    let mut line = Vec::new();
    write_json_prefix(&mut line, key)?;
    let mut cursor: Option<(i64, String)> = None;
    let mut live_count = 0usize;
    loop {
        let page = load_outbound_message_page(
            tx,
            key,
            cursor
                .as_ref()
                .map(|(timestamp, id)| (*timestamp, id.as_str())),
        )
        .await?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for message in page {
            let (count, next_cursor) =
                append_serialized_message(&mut line, key, message, live_count)?;
            live_count = count;
            cursor = Some(next_cursor);
        }
        if page_len < MESSAGE_PAGE_SIZE {
            break;
        }
    }
    let tombstone_count = append_tombstones(tx, key, &mut line, live_count).await?;
    line.write_all(b"]}\n")
        .map_err(|error| format!("Message push suffix failed for {}: {error}", key.topic_id))?;
    Ok(SerializedTopicMessages {
        line,
        live_count,
        tombstone_count,
    })
}

pub(super) async fn serialize_selected_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    selection: &MessagePushSelection,
) -> Result<SerializedTopicMessages, String> {
    let key = &selection.topic;
    let ordered_ids = load_outbound_message_order(tx, key, &selection.message_ids).await?;
    let mut line = Vec::new();
    write_json_prefix(&mut line, key)?;
    let mut live_count = 0usize;
    for chunk in ordered_ids.chunks(MESSAGE_PAGE_SIZE) {
        let page = load_outbound_message_page_by_ids(tx, key, chunk).await?;
        if page.len() != chunk.len() {
            return Err(encode_local_sync_error(
                "SYNC_SNAPSHOT_STALE",
                SyncErrorStage::Messages,
                &format!(
                    "Selected outbound messages for {} changed during snapshot",
                    key.topic_id
                ),
                vec![key.topic_id.clone()],
            ));
        }
        for message in page {
            let (count, _) = append_serialized_message(&mut line, key, message, live_count)?;
            live_count = count;
        }
    }
    let tombstone_count =
        append_selected_tombstones(key, &selection.deleted_messages, &mut line, live_count)?;
    line.write_all(b"]}\n")
        .map_err(|error| format!("Message push suffix failed for {}: {error}", key.topic_id))?;
    Ok(SerializedTopicMessages {
        line,
        live_count,
        tombstone_count,
    })
}

async fn load_outbound_message_order(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    message_ids: &[String],
) -> Result<Vec<String>, String> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    if message_ids.len() > MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {} exceeds the {MAX_MESSAGES_PER_TOPIC}-message limit",
            key.topic_id
        ));
    }
    let mut entries = Vec::with_capacity(message_ids.len());
    for chunk in message_ids.chunks(MESSAGE_PAGE_SIZE) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT timestamp, msg_id
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
               AND msg_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for id in chunk {
            query = query.bind(id);
        }
        let rows = query.fetch_all(&mut **tx).await.map_err(|error| {
            format!(
                "Selected message order query failed for {}: {error}",
                key.topic_id
            )
        })?;
        if rows.len() != chunk.len() {
            return Err(encode_local_sync_error(
                "SYNC_SNAPSHOT_STALE",
                SyncErrorStage::Messages,
                &format!(
                    "Selected outbound messages for {} disappeared or became tombstoned",
                    key.topic_id
                ),
                vec![key.topic_id.clone()],
            ));
        }
        for row in rows {
            let id: String = row.try_get("msg_id").map_err(|error| {
                format!("Message id decode failed for {}: {error}", key.topic_id)
            })?;
            let timestamp = decode_non_negative_i64(&row, "timestamp", &id, key)?;
            entries.push((timestamp, id));
        }
    }
    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(entries.into_iter().map(|(_, id)| id).collect())
}

pub(super) async fn load_outbound_message_page_by_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<Vec<MessageSyncDTO>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT msg_id, role, name, agent_id, content, timestamp,
                is_group_message, group_id, finish_reason, content_hash, updated_at
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
           AND msg_id IN ({placeholders})"
    );
    let mut query = sqlx::query(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&mut **tx).await.map_err(|error| {
        format!(
            "Selected message page query failed for {}: {error}",
            key.topic_id
        )
    })?;
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        messages.push(decode_message_row(&row, key)?);
    }
    messages.sort_unstable_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
    load_message_attachments(tx, key, &mut messages).await
}

fn append_serialized_message(
    line: &mut Vec<u8>,
    key: &TopicKey,
    message: MessageSyncDTO,
    live_count: usize,
) -> Result<(usize, (i64, String)), String> {
    let timestamp = i64::try_from(message.timestamp).map_err(|_| {
        format!(
            "Outbound message {}/{} timestamp exceeds SQLite range",
            key.topic_id, message.id
        )
    })?;
    let next_cursor = (timestamp, message.id.clone());
    if live_count > 0 {
        line.write_all(b",").map_err(|error| {
            format!(
                "Message push separator failed for {}: {error}",
                key.topic_id
            )
        })?;
    }
    serde_json::to_writer(&mut *line, &message).map_err(|error| {
        format!(
            "Message push topic {} contains invalid data: {error}",
            key.topic_id
        )
    })?;
    let count = live_count
        .checked_add(1)
        .ok_or_else(|| "Message push count overflow".to_string())?;
    if count > MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {} exceeds the {MAX_MESSAGES_PER_TOPIC}-message limit",
            key.topic_id
        ));
    }
    Ok((count, next_cursor))
}

fn write_json_prefix(line: &mut Vec<u8>, key: &TopicKey) -> Result<(), String> {
    line.write_all(br#"{"kind":"topic","topicId":"#)
        .map_err(|error| format!("Message push prefix failed for {}: {error}", key.topic_id))?;
    serde_json::to_writer(&mut *line, &key.topic_id)
        .map_err(|error| format!("Message push topic id serialization failed: {error}"))?;
    line.write_all(b",\"ownerType\":").map_err(|error| {
        format!(
            "Message push owner prefix failed for {}: {error}",
            key.topic_id
        )
    })?;
    serde_json::to_writer(&mut *line, &key.owner_type)
        .map_err(|error| format!("Message push owner type serialization failed: {error}"))?;
    line.write_all(b",\"ownerId\":").map_err(|error| {
        format!(
            "Message push owner prefix failed for {}: {error}",
            key.topic_id
        )
    })?;
    serde_json::to_writer(&mut *line, &key.owner_id)
        .map_err(|error| format!("Message push owner id serialization failed: {error}"))?;
    line.write_all(b",\"messages\":[")
        .map_err(|error| format!("Message push prefix failed for {}: {error}", key.topic_id))
}

fn append_selected_tombstones(
    key: &TopicKey,
    deleted_messages: &[MessageDeleteDecision],
    line: &mut Vec<u8>,
    live_count: usize,
) -> Result<usize, String> {
    line.write_all(b"],\"deletedMessages\":[")
        .map_err(|error| {
            format!(
                "Message tombstone prefix failed for {}: {error}",
                key.topic_id
            )
        })?;
    let mut count = 0usize;
    for tombstone in deleted_messages {
        append_tombstone_entry(
            line,
            key,
            &tombstone.msg_id,
            tombstone.deleted_at,
            live_count,
            count,
        )?;
        count = count
            .checked_add(1)
            .ok_or_else(|| "Message tombstone count overflow".to_string())?;
    }
    Ok(count)
}

async fn append_tombstones(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    line: &mut Vec<u8>,
    live_count: usize,
) -> Result<usize, String> {
    line.write_all(b"],\"deletedMessages\":[")
        .map_err(|error| {
            format!(
                "Message tombstone prefix failed for {}: {error}",
                key.topic_id
            )
        })?;
    let rows = load_tombstone_rows(tx, key).await?;
    let mut count = 0usize;
    for row in rows {
        let message_id: String = row.try_get("msg_id").map_err(|error| {
            format!(
                "Message tombstone id decode failed for {}: {error}",
                key.topic_id
            )
        })?;
        let deleted_at: i64 = row.try_get("deleted_at").map_err(|error| {
            format!(
                "Message tombstone timestamp decode failed for {}/{}: {error}",
                key.topic_id, message_id
            )
        })?;
        append_tombstone_entry(line, key, &message_id, deleted_at, live_count, count)?;
        count = count
            .checked_add(1)
            .ok_or_else(|| "Message tombstone count overflow".to_string())?;
    }
    Ok(count)
}

async fn load_tombstone_rows(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    sqlx::query(
        "SELECT msg_id, deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NOT NULL
         ORDER BY deleted_at ASC, msg_id ASC",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| {
        format!(
            "Message tombstone query failed for {}: {error}",
            key.topic_id
        )
    })
}

fn append_tombstone_entry(
    line: &mut Vec<u8>,
    key: &TopicKey,
    message_id: &str,
    deleted_at: i64,
    live_count: usize,
    tombstone_count: usize,
) -> Result<(), String> {
    if message_id.is_empty() || !(0..=MAX_SAFE_TIMESTAMP).contains(&deleted_at) {
        return Err(format!(
            "Message tombstone {}/{} has invalid identity or timestamp",
            key.topic_id, message_id
        ));
    }
    if live_count.saturating_add(tombstone_count) >= MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {} exceeds the message limit",
            key.topic_id
        ));
    }
    if tombstone_count > 0 {
        line.write_all(b",").map_err(|error| {
            format!(
                "Message tombstone separator failed for {}: {error}",
                key.topic_id
            )
        })?;
    }
    let tombstone = serde_json::json!({ "msgId": message_id, "deletedAt": deleted_at });
    serde_json::to_writer(&mut *line, &tombstone).map_err(|error| {
        format!(
            "Message tombstone serialization failed for {}: {error}",
            key.topic_id
        )
    })
}
