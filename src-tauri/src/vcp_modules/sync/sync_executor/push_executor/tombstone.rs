use super::types::{
    BoundedJsonLine, CountingWriter, MessageTombstone, MessageTombstoneRequest, PushBatchResult,
    TopicMessagePreflight, MAX_MESSAGES_PER_TOPIC, MAX_NDJSON_LINE_BYTES,
};
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};

pub(super) async fn preflight_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<TopicMessagePreflight, String> {
    let row = sqlx::query(
        "SELECT
            SUM(CASE WHEN deleted_at IS NULL THEN 1 ELSE 0 END) AS live_count,
            SUM(CASE WHEN deleted_at IS NOT NULL THEN 1 ELSE 0 END) AS tombstone_count,
            COALESCE(SUM(LENGTH(CAST(content AS BLOB))), 0) AS content_bytes
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| {
        format!(
            "Message push preflight failed for {}: {error}",
            key.topic_id
        )
    })?;
    let live_count = count(&row, "live_count", key)?;
    let tombstone_count = count(&row, "tombstone_count", key)?;
    let content_bytes = count(&row, "content_bytes", key)?;
    let attachment_bytes = attachment_bytes(tx, key).await?;
    let total_count = live_count
        .checked_add(tombstone_count)
        .ok_or_else(|| format!("Message count overflow for {}", key.topic_id))?;
    if total_count > MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {} contains {total_count} messages, limit is {MAX_MESSAGES_PER_TOPIC}",
            key.topic_id
        ));
    }
    let estimated = content_bytes
        .checked_add(attachment_bytes)
        .and_then(|value| value.checked_add(tombstone_count.saturating_mul(64)))
        .ok_or_else(|| format!("Message push size overflow for {}", key.topic_id))?;
    if estimated > MAX_NDJSON_LINE_BYTES {
        return Err(format!(
            "Message push topic {} exceeds the 32 MiB line budget",
            key.topic_id
        ));
    }
    Ok(TopicMessagePreflight {
        live_count,
        tombstone_count,
    })
}

async fn attachment_bytes(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<usize, String> {
    let value: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(
            LENGTH(CAST(ma.hash AS BLOB)) + LENGTH(CAST(ma.display_name AS BLOB)) +
            COALESCE(LENGTH(CAST(a.mime_type AS BLOB)), 0) +
            COALESCE(LENGTH(CAST(a.extracted_text AS BLOB)), 0) +
            COALESCE(LENGTH(CAST(a.image_frames AS BLOB)), 0)
         ), 0)
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| {
        format!(
            "Message attachment preflight failed for {}: {error}",
            key.topic_id
        )
    })?;
    usize::try_from(value)
        .map_err(|_| format!("Message attachment size is invalid for {}", key.topic_id))
}

fn count(row: &sqlx::sqlite::SqliteRow, field: &str, key: &TopicKey) -> Result<usize, String> {
    let value: Option<i64> = row.try_get(field).map_err(|error| {
        format!(
            "Message {field} decode failed for {}: {error}",
            key.topic_id
        )
    })?;
    usize::try_from(value.unwrap_or(0))
        .map_err(|_| format!("Message {field} is invalid for {}", key.topic_id))
}

pub(super) async fn load_message_tombstones(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    expected_count: usize,
) -> Result<Vec<MessageTombstone>, String> {
    let rows = sqlx::query(
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
    })?;
    if rows.len() != expected_count {
        return Err(format!(
            "Message tombstones for {} changed during serialization: expected {expected_count}, got {}",
            key.topic_id,
            rows.len()
        ));
    }
    rows.into_iter()
        .map(|row| {
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
            if message_id.is_empty() || deleted_at < 0 {
                return Err(format!(
                    "Message tombstone {}/{} has invalid identity or timestamp",
                    key.topic_id, message_id
                ));
            }
            Ok(MessageTombstone {
                topic: key.clone(),
                message_id,
                deleted_at,
            })
        })
        .collect()
}

pub(super) fn message_tombstone_body_len(tombstone: &MessageTombstone) -> Result<usize, String> {
    let request = MessageTombstoneRequest {
        msg_id: &tombstone.message_id,
        deleted_at: tombstone.deleted_at,
    };
    let mut writer = CountingWriter {
        bytes: 0,
        limit: MAX_NDJSON_LINE_BYTES,
    };
    serde_json::to_writer(&mut writer, &request).map_err(|error| {
        format!(
            "Message tombstone {}/{} serialization failed: {error}",
            tombstone.topic.topic_id, tombstone.message_id
        )
    })?;
    Ok(writer.bytes)
}

pub(super) fn append_topic_failure(result: &mut PushBatchResult, message: String) {
    result.success = false;
    match &mut result.error {
        Some(existing) if !existing.is_empty() => {
            existing.push_str("; ");
            existing.push_str(&message);
        }
        _ => result.error = Some(message),
    }
}

#[allow(dead_code)]
pub(super) fn append_tombstone_for_test(
    line: &mut BoundedJsonLine,
    tombstone: &MessageTombstone,
) -> Result<(), String> {
    serde_json::to_writer(
        line,
        &serde_json::json!({
            "msgId": tombstone.message_id,
            "deletedAt": tombstone.deleted_at,
        }),
    )
    .map_err(|error| format!("tombstone serialization failed: {error}"))
}
