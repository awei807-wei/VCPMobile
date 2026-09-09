use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};

const E2E_PREFIX: &str = "wire14-e2e-";
const TOPIC_PREFIX: &str = "wire14-";
const SCALE_OWNER_PREFIX: &str = "wire14-e2e-scale-owner-";
const MAX_ID_LENGTH: usize = 200;
const MAX_SAFE_TIMESTAMP: i64 = (1_i64 << 53) - 1;

pub(crate) struct MessageFixture {
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) content: String,
    pub(crate) timestamp: u64,
    pub(crate) updated_at: i64,
}

pub(crate) fn validate_target(
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    msg_id: &str,
    attachment_hash: &str,
) -> Result<(), String> {
    if !matches!(owner_type, "agent" | "group") {
        return Err("Wire 1.4 debug fixture ownerType must be agent or group".to_string());
    }
    validate_fixture_id(owner_id, "ownerId", E2E_PREFIX)?;
    validate_fixture_id(topic_id, "topicId", TOPIC_PREFIX)?;
    validate_fixture_id(msg_id, "msgId", E2E_PREFIX)?;
    if attachment_hash.is_empty()
        || attachment_hash.len() > MAX_ID_LENGTH
        || !attachment_hash.bytes().all(is_safe_fixture_byte)
        || is_sha256(attachment_hash)
    {
        return Err(
            "Wire 1.4 debug fixture attachmentHash must be a bounded malformed CAS hash"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_fixture_id(value: &str, field: &str, prefix: &str) -> Result<(), String> {
    if value.len() <= prefix.len()
        || value.len() > MAX_ID_LENGTH
        || !value.starts_with(prefix)
        || !value.bytes().all(is_safe_fixture_byte)
    {
        return Err(format!(
            "Wire 1.4 debug fixture {field} must stay inside the {prefix} namespace"
        ));
    }
    Ok(())
}

fn is_safe_fixture_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn validate_scale_owner(owner_type: &str, owner_id: &str) -> Result<(), String> {
    if owner_type != "agent" {
        return Err(
            "Wire 1.4 debug scale hash ownerType must be agent synthetic owner".to_string(),
        );
    }
    validate_fixture_id(owner_id, "ownerId", SCALE_OWNER_PREFIX)
}

pub(crate) async fn load_scale_topic_hashes(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
) -> Result<Vec<super::Wire14DebugTopicHash>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, config_hash, content_hash
         FROM topics
         WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL
         ORDER BY topic_id ASC",
    )
    .bind(owner_type)
    .bind(owner_id)
    .fetch_all(pool)
    .await
    .map_err(|error| format!("Read debug scale topic hashes failed: {error}"))?;

    rows.into_iter()
        .map(|row| {
            Ok(super::Wire14DebugTopicHash {
                id: row
                    .try_get("topic_id")
                    .map_err(|error| format!("Decode debug scale topic id failed: {error}"))?,
                config_hash: row.try_get("config_hash").map_err(|error| {
                    format!("Decode debug scale topic config hash failed: {error}")
                })?,
                content_hash: row.try_get("content_hash").map_err(|error| {
                    format!("Decode debug scale topic content hash failed: {error}")
                })?,
            })
        })
        .collect()
}

pub(crate) fn next_update_clock(previous: i64) -> Result<i64, String> {
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&previous) {
        return Err("Wire 1.4 debug fixture message updatedAt is invalid".to_string());
    }
    let now = chrono::Utc::now().timestamp_millis();
    let next = now.max(previous.saturating_add(1));
    if next > MAX_SAFE_TIMESTAMP {
        return Err("Wire 1.4 debug fixture update clock exceeds safe integer range".to_string());
    }
    Ok(next)
}

pub(crate) async fn ensure_live_owner(
    tx: &mut Transaction<'_, Sqlite>,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    let (table, id_column) = match owner_type {
        "agent" => ("agents", "agent_id"),
        "group" => ("groups", "group_id"),
        _ => return Err("unsupported debug fixture owner type".to_string()),
    };
    let query = format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE {id_column} = ? AND deleted_at IS NULL)"
    );
    let exists: bool = sqlx::query_scalar(&query)
        .bind(owner_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| format!("Read debug fixture owner failed: {error}"))?;
    if !exists {
        return Err("Wire 1.4 debug fixture owner is missing or deleted".to_string());
    }
    Ok(())
}

pub(crate) async fn ensure_live_topic(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM topics
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
        )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Read debug fixture topic failed: {error}"))?;
    if !exists {
        return Err("Wire 1.4 debug fixture topic is missing or deleted".to_string());
    }
    Ok(())
}

pub(crate) async fn load_live_message(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_id: &str,
) -> Result<MessageFixture, String> {
    let row = sqlx::query(
        "SELECT role, name, agent_id, content, timestamp, updated_at
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("Read debug fixture message failed: {error}"))?
    .ok_or_else(|| "Wire 1.4 debug fixture message is missing or deleted".to_string())?;
    let timestamp = decode_safe_timestamp(&row, "timestamp")?;
    let updated_at = decode_safe_timestamp_i64(&row, "updated_at")?;
    Ok(MessageFixture {
        role: row
            .try_get("role")
            .map_err(|error| format!("Decode debug fixture role failed: {error}"))?,
        name: row
            .try_get("name")
            .map_err(|error| format!("Decode debug fixture name failed: {error}"))?,
        agent_id: row
            .try_get("agent_id")
            .map_err(|error| format!("Decode debug fixture agent id failed: {error}"))?,
        content: decode_message_content(&row, "content")?,
        timestamp,
        updated_at,
    })
}

fn decode_safe_timestamp(row: &sqlx::sqlite::SqliteRow, field: &str) -> Result<u64, String> {
    let value = decode_safe_timestamp_i64(row, field)?;
    u64::try_from(value).map_err(|_| format!("Debug fixture {field} is negative"))
}

fn decode_safe_timestamp_i64(row: &sqlx::sqlite::SqliteRow, field: &str) -> Result<i64, String> {
    let value: i64 = row
        .try_get(field)
        .map_err(|error| format!("Decode debug fixture {field} failed: {error}"))?;
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&value) {
        return Err(format!(
            "Debug fixture {field} is outside safe integer range"
        ));
    }
    Ok(value)
}

pub(crate) async fn ensure_relation_is_new(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    attachment_hash: &str,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM message_attachments
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
              AND hash = ? AND deleted_at IS NULL
        )",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .bind(attachment_hash)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Read debug attachment relation failed: {error}"))?;
    if exists {
        return Err("Wire 1.4 debug fixture attachment relation already exists".to_string());
    }
    let catalog_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments WHERE hash = ?)")
            .bind(attachment_hash)
            .fetch_one(&mut **tx)
            .await
            .map_err(|error| format!("Read debug attachment catalog failed: {error}"))?;
    if catalog_exists {
        return Err("Wire 1.4 debug fixture attachment catalog entry already exists".to_string());
    }
    Ok(())
}

pub(crate) async fn next_attachment_order(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_id: &str,
) -> Result<i32, String> {
    let max_order: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(attachment_order) FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("Read debug attachment order failed: {error}"))?;
    max_order
        .unwrap_or(-1)
        .checked_add(1)
        .ok_or_else(|| "Wire 1.4 debug fixture attachment order overflowed".to_string())
}

pub(crate) async fn load_attachment_hashes(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_id: &str,
) -> Result<Vec<String>, String> {
    sqlx::query_scalar(
        "SELECT hash FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL
         ORDER BY attachment_order ASC",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Read debug attachment hashes failed: {error}"))
}

pub(crate) async fn insert_invalid_attachment_catalog(
    tx: &mut Transaction<'_, Sqlite>,
    attachment_hash: &str,
    timestamp: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO attachments (
            hash, mime_type, size, internal_path, extracted_text,
            image_frames, thumbnail_path, created_at, updated_at
         ) VALUES (?, 'application/octet-stream', 1, '', NULL, NULL, NULL, ?, ?)",
    )
    .bind(attachment_hash)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Insert debug attachment catalog failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_fixture_gate_accepts_only_malformed_e2e_target() {
        assert!(validate_target(
            "agent",
            "wire14-e2e-owner-a-run",
            "wire14-attachment-topic-run",
            "wire14-e2e-invalid-message-run",
            "not-a-sha256",
        )
        .is_ok());
        assert!(validate_target(
            "agent",
            "owner-a",
            "wire14-attachment-topic-run",
            "wire14-e2e-invalid-message-run",
            "not-a-sha256",
        )
        .is_err());
        assert!(validate_target(
            "agent",
            "wire14-e2e-owner-a-run",
            "wire14-attachment-topic-run",
            "wire14-e2e-invalid-message-run",
            &"a".repeat(64),
        )
        .is_err());
    }

    #[test]
    fn debug_fixture_hash_gate_rejects_control_characters() {
        assert!(validate_target(
            "agent",
            "wire14-e2e-owner-a-run",
            "wire14-topic-run",
            "wire14-e2e-message-run",
            "invalid/hash",
        )
        .is_err());
    }

    #[test]
    fn scale_hash_gate_allows_only_agent_scale_owner_namespace() {
        assert!(validate_scale_owner("agent", "wire14-e2e-scale-owner-run-123",).is_ok());
        assert!(validate_scale_owner("group", "wire14-e2e-scale-owner-run-123").is_err());
        assert!(validate_scale_owner("agent", "wire14-e2e-owner-a-run-123").is_err());
        assert!(validate_scale_owner("agent", "wire14-e2e-scale-owner-").is_err());
    }
}
