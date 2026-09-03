use super::message_repository_render::ContentCompressor;
use super::message_repository_support::resolve_message_updated_at;
use super::{ExistingMessageState, MessageRepository};
#[path = "message_repository_upsert_decode.rs"]
mod decode;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use decode::decode_existing_message;
use sqlx::Row;

impl MessageRepository {
    async fn resolve_legacy_topic_key(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        topic_id: &str,
    ) -> Result<TopicKey, String> {
        if topic_id.is_empty() {
            return Err("message write requires a non-empty topic id".to_string());
        }
        let rows = sqlx::query(
            "SELECT owner_type, owner_id
             FROM topics WHERE topic_id = ? ORDER BY owner_type, owner_id",
        )
        .bind(topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("resolve topic {topic_id} failed: {error}"))?;
        match rows.as_slice() {
            [row] => Ok(TopicKey::new(
                row.try_get::<String, _>("owner_type")
                    .map_err(|error| error.to_string())?,
                row.try_get::<String, _>("owner_id")
                    .map_err(|error| error.to_string())?,
                topic_id,
            )),
            [] => Err(format!("topic {topic_id} is missing")),
            _ => Err(format!(
                "topic {topic_id} is ambiguous; composite owner identity is required"
            )),
        }
    }

    /// Compatibility facade for old topic-only callers; ambiguous topics fail closed.
    pub async fn upsert_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        topic_id: &str,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String> {
        let key = Self::resolve_legacy_topic_key(tx, topic_id).await?;
        Self::upsert_message_for_topic(tx, message, &key, render_content, skip_bubble).await
    }

    async fn load_upsert_target_state(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        key: &TopicKey,
        msg_id: &str,
    ) -> Result<Option<ExistingMessageState>, String> {
        ensure_topic_is_live(tx, key).await?;
        let row = sqlx::query(
            "SELECT role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash, updated_at, deleted_at
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(msg_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        let Some(row) = row else { return Ok(None) };
        let existing = decode_existing_message(&row)?;
        if existing.deleted_at.is_some() {
            return Err(format!(
                "message {msg_id} is tombstoned and cannot be restored by upsert"
            ));
        }
        Ok(Some(existing))
    }

    /// Upsert one message under its complete composite topic identity.
    pub async fn upsert_message_for_topic(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        message: &ChatMessage,
        key: &TopicKey,
        render_content: &[u8],
        skip_bubble: bool,
    ) -> Result<(), String> {
        let prepared = prepare_upsert(tx, message, key).await?;
        if prepared.core_changed {
            write_message_row(tx, message, key, &prepared).await?;
        }
        write_render_cache(tx, key, render_content, &prepared).await?;
        if prepared.content_changed {
            update_message_fts(tx, key, message, &prepared.message_key).await?;
        }
        persist_message_attachments(tx, key, message, &prepared).await?;
        if !skip_bubble && prepared.fingerprint_changed {
            HashAggregator::bubble_from_topic_for_key(tx, key).await?;
        }
        Ok(())
    }
}

struct PreparedUpsert {
    message_key: MessageKey,
    content_hash: String,
    effective_updated_at: i64,
    message_timestamp: i64,
    content_changed: bool,
    fingerprint_changed: bool,
    core_changed: bool,
}

fn compute_upsert_content_hash(message: &ChatMessage, message_key: &MessageKey) -> String {
    let attachment_hashes = message
        .attachments
        .as_ref()
        .map(|attachments| {
            attachments
                .iter()
                .filter_map(|attachment| attachment.hash.clone())
                .filter(|hash| !hash.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    HashAggregator::compute_message_fingerprint_with_identity(
        &message_key.msg_id,
        &message.role,
        message.name.as_deref(),
        &message.content,
        message.timestamp,
        message.agent_id.as_deref(),
        &attachment_hashes,
    )
}

fn compute_upsert_change_flags(
    message: &ChatMessage,
    existing: Option<&ExistingMessageState>,
    content_hash: &str,
    effective_updated_at: i64,
    message_timestamp: i64,
) -> (bool, bool, bool) {
    let is_group_message = message.is_group_message.unwrap_or(false);
    let content_changed = existing.is_none_or(|state| state.content != message.content);
    let fingerprint_changed = existing.is_none_or(|state| state.content_hash != content_hash);
    let core_changed = existing.is_none_or(|state| {
        state.role != message.role
            || state.name != message.name
            || state.agent_id != message.agent_id
            || state.content != message.content
            || state.timestamp != message_timestamp
            || state.is_group_message != is_group_message
            || state.group_id != message.group_id
            || state.finish_reason != message.finish_reason
            || state.content_hash != content_hash
            || state.updated_at != effective_updated_at
    });
    (content_changed, fingerprint_changed, core_changed)
}

async fn prepare_upsert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &ChatMessage,
    key: &TopicKey,
) -> Result<PreparedUpsert, String> {
    if !key.is_valid() || message.id.is_empty() || message.role.is_empty() {
        return Err("message write has invalid topic/message identity".to_string());
    }
    let message_key = MessageKey::new(key.clone(), message.id.clone());
    let content_hash = compute_upsert_content_hash(message, &message_key);
    let existing =
        MessageRepository::load_upsert_target_state(tx, key, &message_key.msg_id).await?;
    let effective_updated_at = resolve_message_updated_at(
        message.updated_at,
        message.timestamp,
        &content_hash,
        existing
            .as_ref()
            .map(|state| (state.content_hash.as_str(), state.updated_at)),
        chrono::Utc::now().timestamp_millis(),
    )?;
    let message_timestamp = i64::try_from(message.timestamp)
        .map_err(|_| format!("message {} timestamp is too large", message.id))?;
    let (content_changed, fingerprint_changed, core_changed) = compute_upsert_change_flags(
        message,
        existing.as_ref(),
        &content_hash,
        effective_updated_at,
        message_timestamp,
    );
    Ok(PreparedUpsert {
        message_key,
        content_hash,
        effective_updated_at,
        message_timestamp,
        content_changed,
        fingerprint_changed,
        core_changed,
    })
}

async fn write_message_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    message: &ChatMessage,
    key: &TopicKey,
    prepared: &PreparedUpsert,
) -> Result<(), String> {
    let compressed = ContentCompressor::compress(&message.content)?;
    let result = sqlx::query(
        "INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content,
            timestamp, is_group_message, group_id, finish_reason, content_hash,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
            content = excluded.content, role = excluded.role, name = excluded.name,
            agent_id = excluded.agent_id, timestamp = excluded.timestamp,
            is_group_message = excluded.is_group_message, group_id = excluded.group_id,
            finish_reason = excluded.finish_reason, content_hash = excluded.content_hash,
            updated_at = excluded.updated_at
         WHERE messages.deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&prepared.message_key.msg_id)
    .bind(&message.role)
    .bind(&message.name)
    .bind(&message.agent_id)
    .bind(compressed)
    .bind(prepared.message_timestamp)
    .bind(message.is_group_message.unwrap_or(false))
    .bind(&message.group_id)
    .bind(&message.finish_reason)
    .bind(&prepared.content_hash)
    .bind(prepared.message_timestamp)
    .bind(prepared.effective_updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(format!(
            "message {}/{} disappeared during upsert",
            key.topic_id, prepared.message_key.msg_id
        ))
    }
}

async fn write_render_cache(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    render_content: &[u8],
    prepared: &PreparedUpsert,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO render_cache (
            owner_type, owner_id, topic_id, msg_id, render_content, content_hash,
            renderer_schema_version, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
            render_content = excluded.render_content, content_hash = excluded.content_hash,
            renderer_schema_version = excluded.renderer_schema_version,
            updated_at = excluded.updated_at
         WHERE render_cache.render_content IS NOT excluded.render_content
            OR render_cache.content_hash IS NOT excluded.content_hash
            OR render_cache.renderer_schema_version IS NOT excluded.renderer_schema_version
            OR render_cache.updated_at IS NOT excluded.updated_at",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&prepared.message_key.msg_id)
    .bind(render_content)
    .bind(&prepared.content_hash)
    .bind(super::message_repository_render::RENDERER_SCHEMA_VERSION)
    .bind(prepared.effective_updated_at)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn update_message_fts(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    message: &ChatMessage,
    message_key: &MessageKey,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM messages_fts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&message_key.msg_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&message_key.msg_id)
    .bind(&key.topic_id)
    .bind(&message.content)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn persist_message_attachments(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    message: &ChatMessage,
    prepared: &PreparedUpsert,
) -> Result<(), String> {
    if let Some(attachments) = &message.attachments {
        MessageRepository::upsert_attachments_for_message(
            tx,
            key,
            &prepared.message_key.msg_id,
            prepared.message_timestamp,
            attachments,
        )
        .await
    } else {
        sqlx::query(
            "DELETE FROM message_attachments
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(&prepared.message_key.msg_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
}

async fn ensure_topic_is_live(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    let topic_is_live: bool = sqlx::query_scalar(
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
    .map_err(|error| error.to_string())?;
    if topic_is_live {
        Ok(())
    } else {
        Err(format!("topic {} is deleted or missing", key.topic_id))
    }
}
