use super::http::{parse_success_response, require_exact_object_keys};
use super::types::{
    CountingWriter, MessageTombstone, MessageTombstoneRequest, PushBatchResult,
    TopicMessagePreflight, MAX_CONTROL_RESPONSE_BYTES, MAX_MESSAGES_PER_TOPIC,
    MAX_NDJSON_LINE_BYTES,
};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashMap;

pub(super) async fn preflight_topic_messages(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
) -> Result<TopicMessagePreflight, String> {
    let (live_count, message_bytes) = load_live_message_stats(tx, topic_id).await?;
    let attachment_bytes = load_attachment_bytes(tx, topic_id).await?;
    let (tombstone_count, tombstone_bytes) = load_tombstone_stats(tx, topic_id).await?;
    validate_preflight_budget(
        topic_id,
        live_count,
        tombstone_count,
        message_bytes,
        attachment_bytes,
        tombstone_bytes,
    )
}

async fn load_live_message_stats(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
) -> Result<(usize, usize), String> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS message_count,
                COALESCE(SUM(
                    LENGTH(CAST(msg_id AS BLOB)) + LENGTH(CAST(role AS BLOB)) +
                    COALESCE(LENGTH(CAST(name AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(agent_id AS BLOB)), 0) +
                    LENGTH(CAST(content AS BLOB)) +
                    COALESCE(LENGTH(CAST(group_id AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(finish_reason AS BLOB)), 0) +
                    LENGTH(CAST(content_hash AS BLOB))
                ), 0) AS raw_bytes
         FROM messages
         WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message push preflight failed for {topic_id}: {error}"))?;
    let live_count = decode_count(&row, "message_count", topic_id, "Message push count")?;
    let message_bytes = decode_count(&row, "raw_bytes", topic_id, "Message push size")?;
    Ok((live_count, message_bytes))
}

async fn load_attachment_bytes(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
) -> Result<usize, String> {
    let attachment_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(
                    LENGTH(CAST(ma.hash AS BLOB)) + LENGTH(CAST(ma.display_name AS BLOB)) +
                    COALESCE(LENGTH(CAST(ma.src AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(ma.status AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.mime_type AS BLOB)), 0) +
                    COALESCE(LENGTH(CAST(a.image_frames AS BLOB)), 0)
                ), 0)
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         JOIN messages m ON m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
         WHERE ma.topic_id = ? AND ma.deleted_at IS NULL AND m.deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message attachment preflight failed for {topic_id}: {error}"))?;
    usize::try_from(attachment_bytes)
        .map_err(|_| format!("Message attachment size is invalid for {topic_id}"))
}

async fn load_tombstone_stats(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
) -> Result<(usize, usize), String> {
    let tombstone_row = sqlx::query(
        "SELECT COUNT(*) AS tombstone_count,
                COALESCE(SUM(LENGTH(CAST(msg_id AS BLOB)) + 64), 0) AS raw_bytes
         FROM messages
         WHERE topic_id = ? AND deleted_at IS NOT NULL",
    )
    .bind(topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Message tombstone preflight failed for {topic_id}: {error}"))?;
    let tombstone_count = decode_count(
        &tombstone_row,
        "tombstone_count",
        topic_id,
        "Message tombstone count",
    )?;
    let tombstone_bytes = decode_count(
        &tombstone_row,
        "raw_bytes",
        topic_id,
        "Message tombstone size",
    )?;
    Ok((tombstone_count, tombstone_bytes))
}

fn validate_preflight_budget(
    topic_id: &str,
    live_count: usize,
    tombstone_count: usize,
    message_bytes: usize,
    attachment_bytes: usize,
    tombstone_bytes: usize,
) -> Result<TopicMessagePreflight, String> {
    let total_count = live_count
        .checked_add(tombstone_count)
        .ok_or_else(|| format!("Message count overflow for {topic_id}"))?;
    if total_count > MAX_MESSAGES_PER_TOPIC {
        return Err(format!(
            "Message push topic {topic_id} contains {total_count} live messages and tombstones, limit is {MAX_MESSAGES_PER_TOPIC}"
        ));
    }
    let raw_bytes = message_bytes
        .checked_add(attachment_bytes)
        .and_then(|bytes| bytes.checked_add(tombstone_bytes))
        .ok_or_else(|| format!("Message push size overflow for {topic_id}"))?;
    if raw_bytes > MAX_NDJSON_LINE_BYTES {
        return Err(format!(
            "Message push topic {topic_id} exceeds the 32 MiB line limit before serialization"
        ));
    }
    Ok(TopicMessagePreflight {
        live_count,
        tombstone_count,
    })
}

fn decode_count(
    row: &sqlx::sqlite::SqliteRow,
    field: &str,
    topic_id: &str,
    label: &str,
) -> Result<usize, String> {
    let value: i64 = row
        .try_get(field)
        .map_err(|error| format!("{label} decode failed for {topic_id}: {error}"))?;
    usize::try_from(value).map_err(|_| format!("{label} is invalid for {topic_id}"))
}

pub(super) async fn load_message_tombstones(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    expected_count: usize,
) -> Result<Vec<MessageTombstone>, String> {
    let rows = sqlx::query(
        "SELECT msg_id, deleted_at FROM messages
         WHERE topic_id = ? AND deleted_at IS NOT NULL
         ORDER BY deleted_at ASC, msg_id ASC",
    )
    .bind(topic_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Message tombstone query failed for {topic_id}: {error}"))?;
    if rows.len() != expected_count {
        return Err(format!(
            "Message tombstones for {topic_id} changed during serialization: expected {expected_count}, got {}",
            rows.len()
        ));
    }
    rows.into_iter()
        .map(|row| {
            let message_id: String = row.try_get("msg_id").map_err(|error| {
                format!("Message tombstone id decode failed for {topic_id}: {error}")
            })?;
            let deleted_at: i64 = row.try_get("deleted_at").map_err(|error| {
                format!("Message tombstone timestamp decode failed for {topic_id}/{message_id}: {error}")
            })?;
            if message_id.is_empty() || deleted_at < 0 {
                return Err(format!("Message tombstone {topic_id}/{message_id} has an invalid identity or timestamp"));
            }
            Ok(MessageTombstone {
                topic_id: topic_id.to_string(),
                message_id,
                deleted_at,
            })
        })
        .collect()
}

pub(super) fn message_tombstone_body_len(tombstone: &MessageTombstone) -> Result<usize, String> {
    let request = MessageTombstoneRequest {
        topic_id: &tombstone.topic_id,
        msg_id: &tombstone.message_id,
        deleted_at: tombstone.deleted_at,
    };
    let mut writer = CountingWriter {
        bytes: 0,
        limit: MAX_CONTROL_RESPONSE_BYTES,
    };
    serde_json::to_writer(&mut writer, &request).map_err(|error| {
        format!(
            "Message tombstone {}/{} exceeds its serialized byte budget: {error}",
            tombstone.topic_id, tombstone.message_id
        )
    })?;
    Ok(writer.bytes)
}

pub(super) fn validate_message_tombstone_response(
    value: &serde_json::Value,
    tombstone: &MessageTombstone,
) -> Result<(), String> {
    let object =
        require_exact_object_keys(value, &["success", "topicId", "msgId"], "Delete message")?;
    if object.get("success").and_then(serde_json::Value::as_bool) != Some(true)
        || object.get("topicId").and_then(serde_json::Value::as_str)
            != Some(tombstone.topic_id.as_str())
        || object.get("msgId").and_then(serde_json::Value::as_str)
            != Some(tombstone.message_id.as_str())
    {
        return Err(format!(
            "Delete message response requires matching topicId and msgId for {}/{}",
            tombstone.topic_id, tombstone.message_id
        ));
    }
    Ok(())
}

pub(super) async fn push_message_tombstone(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    tombstone: &MessageTombstone,
) -> Result<(), String> {
    let request = MessageTombstoneRequest {
        topic_id: &tombstone.topic_id,
        msg_id: &tombstone.message_id,
        deleted_at: tombstone.deleted_at,
    };
    let body = serde_json::to_vec(&request).map_err(|error| {
        format!(
            "Message tombstone {}/{} serialization failed: {error}",
            tombstone.topic_id, tombstone.message_id
        )
    })?;
    if body.len() > MAX_CONTROL_RESPONSE_BYTES {
        return Err(format!(
            "Message tombstone {}/{} exceeds the request byte limit",
            tombstone.topic_id, tombstone.message_id
        ));
    }
    let idempotency_key = crate::vcp_modules::infra::utils::calculate_sha256_slices(&[
        b"delete-message",
        tombstone.topic_id.as_bytes(),
        tombstone.message_id.as_bytes(),
        tombstone.deleted_at.to_string().as_bytes(),
    ]);
    let response = client
        .post(format!("{http_url}/api/mobile-sync/delete-message"))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("x-idempotency-key", idempotency_key)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| {
            format!(
                "Delete message {}/{} request failed: {error}",
                tombstone.topic_id, tombstone.message_id
            )
        })?;
    let value = parse_success_response(response, "Delete message").await?;
    validate_message_tombstone_response(&value, tombstone)
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

pub(super) async fn push_message_tombstone_chunk(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    tombstones: Vec<MessageTombstone>,
    results: &mut [PushBatchResult],
) -> Result<(), String> {
    if tombstones.is_empty() {
        return Ok(());
    }
    let indexes = results
        .iter()
        .enumerate()
        .map(|(index, result)| (result.topic_id.clone(), index))
        .collect::<HashMap<_, _>>();
    const MAX_CONCURRENT_MESSAGE_DELETES: usize = 3;
    for chunk in tombstones.chunks(MAX_CONCURRENT_MESSAGE_DELETES) {
        let futures = chunk
            .iter()
            .map(|tombstone| push_message_tombstone(client, http_url, sync_token, tombstone));
        for (tombstone, outcome) in chunk
            .iter()
            .zip(futures_util::future::join_all(futures).await)
        {
            if let Err(error) = outcome {
                let index = indexes.get(&tombstone.topic_id).copied().ok_or_else(|| {
                    format!(
                        "Message tombstone result references missing topic {}",
                        tombstone.topic_id
                    )
                })?;
                append_topic_failure(
                    &mut results[index],
                    format!("Message tombstone {} failed: {error}", tombstone.message_id),
                );
            }
        }
    }
    Ok(())
}
