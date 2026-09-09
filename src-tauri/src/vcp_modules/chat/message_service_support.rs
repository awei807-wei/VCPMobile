use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::db_manager::require_db_state;
use crate::vcp_modules::file_manager::resolve_attachment_cas_file;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, RENDERER_SCHEMA_VERSION};
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::Row;
use std::collections::HashMap;
use tauri::AppHandle;

pub(crate) fn topic_key(
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
) -> Result<TopicKey, String> {
    let key = TopicKey::new(owner_type, owner_id, topic_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err(format!(
            "invalid topic identity: ownerType={owner_type}, ownerId={owner_id}, topicId={topic_id}"
        ))
    }
}

/// Resolve a pre-Wire-1.4 topic-only reference only when it is unambiguous.
pub(crate) async fn resolve_unique_topic_key(
    pool: &sqlx::SqlitePool,
    topic_id: &str,
) -> Result<TopicKey, String> {
    if topic_id.is_empty() {
        return Err("topic id is required".to_string());
    }
    let rows = sqlx::query(
        "SELECT owner_type, owner_id
         FROM topics
         WHERE topic_id = ?
         ORDER BY owner_type, owner_id",
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|error| format!("resolve topic {topic_id} failed: {error}"))?;
    match rows.as_slice() {
        [row] => {
            let owner_type: String = row
                .try_get("owner_type")
                .map_err(|error| format!("decode topic owner type failed: {error}"))?;
            let owner_id: String = row
                .try_get("owner_id")
                .map_err(|error| format!("decode topic owner id failed: {error}"))?;
            topic_key(&owner_id, &owner_type, topic_id)
        }
        [] => Err(format!("topic {topic_id} not found")),
        _ => Err(format!(
            "topic {topic_id} is ambiguous; ownerType and ownerId are required"
        )),
    }
}

/// Resolve a message-only reference only when its complete identity is unique.
pub(crate) async fn resolve_unique_message_key(
    pool: &sqlx::SqlitePool,
    message_id: &str,
) -> Result<MessageKey, String> {
    if message_id.is_empty() {
        return Err("message id is required".to_string());
    }
    let rows = sqlx::query(
        "SELECT owner_type, owner_id, topic_id
         FROM messages
         WHERE msg_id = ?
         ORDER BY owner_type, owner_id, topic_id",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
    .map_err(|error| format!("resolve message {message_id} failed: {error}"))?;
    match rows.as_slice() {
        [row] => {
            let owner_type: String = row
                .try_get("owner_type")
                .map_err(|error| format!("decode message owner type failed: {error}"))?;
            let owner_id: String = row
                .try_get("owner_id")
                .map_err(|error| format!("decode message owner id failed: {error}"))?;
            let topic_id: String = row
                .try_get("topic_id")
                .map_err(|error| format!("decode message topic id failed: {error}"))?;
            Ok(MessageKey::new(
                topic_key(&owner_id, &owner_type, &topic_id)?,
                message_id,
            ))
        }
        [] => Err(format!("message {message_id} not found")),
        _ => Err(format!(
            "message {message_id} is ambiguous; complete topic identity is required"
        )),
    }
}

pub(crate) fn attachment_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Attachment, String> {
    let size: i64 = row.try_get("size").map_err(|error| error.to_string())?;
    let created_at: i64 = row
        .try_get("created_at")
        .map_err(|error| error.to_string())?;
    let status: Option<String> = row.try_get("status").map_err(|error| error.to_string())?;
    let attachment_order: i32 = row
        .try_get("attachment_order")
        .map_err(|error| error.to_string())?;
    Ok(Attachment {
        r#type: row
            .try_get("mime_type")
            .map_err(|error| error.to_string())?,
        src: row.try_get("src").map_err(|error| error.to_string())?,
        name: row
            .try_get("display_name")
            .map_err(|error| error.to_string())?,
        size: u64::try_from(size).map_err(|_| "attachment size is negative".to_string())?,
        hash: Some(row.try_get("hash").map_err(|error| error.to_string())?),
        status,
        attachment_order: Some(attachment_order),
        internal_path: row
            .try_get("internal_path")
            .map_err(|error| error.to_string())?,
        extracted_text: row
            .try_get("extracted_text")
            .map_err(|error| error.to_string())?,
        image_frames: row
            .try_get::<Option<String>, _>("image_frames")
            .map_err(|error| error.to_string())?
            .and_then(|value| serde_json::from_str(&value).ok()),
        thumbnail_path: row
            .try_get("thumbnail_path")
            .map_err(|error| error.to_string())?,
        created_at: Some(
            u64::try_from(created_at)
                .map_err(|_| "attachment created_at is negative".to_string())?,
        ),
    })
}

pub(crate) async fn load_attachments_for_topic(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    msg_ids: &[String],
    include_extracted_text: bool,
) -> Result<HashMap<String, Vec<Attachment>>, String> {
    let mut result = HashMap::new();
    if msg_ids.is_empty() {
        return Ok(result);
    }
    let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let extracted_text_column = if include_extracted_text {
        "a.extracted_text"
    } else {
        "NULL"
    };
    let query_string = format!(
        "SELECT a.hash, a.mime_type, a.size, a.internal_path,
                {extracted_text_column} AS extracted_text,
                a.image_frames, a.thumbnail_path, a.created_at,
                ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id,
                ma.attachment_order, ma.display_name, ma.src, ma.status
         FROM message_attachments ma
         JOIN attachments a ON ma.hash = a.hash
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.msg_id IN ({placeholders}) AND ma.deleted_at IS NULL
         ORDER BY ma.msg_id, ma.attachment_order ASC"
    );
    let mut query = sqlx::query(&query_string)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        query = query.bind(msg_id);
    }
    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    for row in rows {
        let msg_id: String = row.try_get("msg_id").map_err(|error| error.to_string())?;
        let mut attachment = attachment_from_row(&row)?;
        if include_extracted_text && attachment.extracted_text.is_none() {
            attachment.extracted_text =
                crate::vcp_modules::infra::file_manager::ensure_extracted_text(
                    pool,
                    attachment.hash.as_deref().unwrap_or_default(),
                    &attachment.internal_path,
                    &attachment.r#type,
                )
                .await;
        }
        result
            .entry(msg_id)
            .or_insert_with(Vec::new)
            .push(attachment);
    }
    Ok(result)
}

pub(crate) async fn load_attachments_for_message_keys(
    pool: &sqlx::SqlitePool,
    keys: &[MessageKey],
) -> Result<HashMap<MessageKey, Vec<Attachment>>, String> {
    let mut result = HashMap::new();
    if keys.is_empty() {
        return Ok(result);
    }
    let tuples = keys
        .iter()
        .map(|_| "(?, ?, ?, ?)")
        .collect::<Vec<_>>()
        .join(",");
    let query_string = format!(
        "SELECT a.hash, a.mime_type, a.size, a.internal_path, NULL AS extracted_text,
                a.image_frames, a.thumbnail_path, a.created_at,
                ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id,
                ma.attachment_order, ma.display_name, ma.src, ma.status
         FROM message_attachments ma
         JOIN attachments a ON ma.hash = a.hash
         WHERE (ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id) IN ({tuples})
           AND ma.deleted_at IS NULL
         ORDER BY ma.owner_type, ma.owner_id, ma.topic_id, ma.msg_id,
                  ma.attachment_order ASC"
    );
    let mut query = sqlx::query(&query_string);
    for key in keys {
        query = query
            .bind(&key.topic.owner_type)
            .bind(&key.topic.owner_id)
            .bind(&key.topic.topic_id)
            .bind(&key.msg_id);
    }
    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
    for row in rows {
        let owner_type: String = row
            .try_get("owner_type")
            .map_err(|error| error.to_string())?;
        let owner_id: String = row.try_get("owner_id").map_err(|error| error.to_string())?;
        let topic_id: String = row.try_get("topic_id").map_err(|error| error.to_string())?;
        let msg_id: String = row.try_get("msg_id").map_err(|error| error.to_string())?;
        let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), msg_id);
        result
            .entry(key)
            .or_insert_with(Vec::new)
            .push(attachment_from_row(&row)?);
    }
    Ok(result)
}

/// Bind attachment metadata only to a file already verified in the local CAS.
/// Wire 1.4 transfers metadata, never an implicit unverified network blob.
pub(crate) async fn ensure_attachments_locally<R: tauri::Runtime>(
    app: &AppHandle<R>,
    message: &mut ChatMessage,
) -> Result<(), String> {
    let attachments = match message.attachments.as_mut() {
        Some(attachments) => attachments,
        None => return Ok(()),
    };
    let db = require_db_state(app)?;
    for attachment in attachments {
        let hash = match attachment.hash.as_deref() {
            Some(hash) if crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) => hash,
            Some(_) => return Err("附件包含非法的 CAS SHA-256 哈希".to_string()),
            None => continue,
        };
        match resolve_attachment_cas_file(app, &db.pool, hash).await {
            Ok(file) => {
                let path = file.path.to_string_lossy().into_owned();
                attachment.src = format!("file://{path}");
                attachment.internal_path = path;
                attachment.status = Some("ready".to_string());
            }
            Err(_) => {
                attachment.src.clear();
                attachment.internal_path.clear();
                attachment.status = Some("desktop_only".to_string());
            }
        }
    }
    Ok(())
}

pub(crate) fn parse_render_bytes(render_content: Option<Vec<u8>>) -> Option<serde_json::Value> {
    render_content.and_then(|bytes| {
        MessageRenderCompiler::deserialize(&bytes)
            .ok()
            .and_then(|blocks| serde_json::to_value(blocks).ok())
    })
}

pub(crate) fn resolve_render_blocks(
    content: &str,
    expected_hash: &str,
    render_content: Option<Vec<u8>>,
    cached_hash: Option<&str>,
    cached_schema_version: Option<i64>,
) -> (Option<serde_json::Value>, Option<Vec<u8>>) {
    let cache_matches = !expected_hash.is_empty()
        && cached_hash == Some(expected_hash)
        && cached_schema_version == Some(RENDERER_SCHEMA_VERSION);
    if cache_matches {
        if let Some(blocks) = parse_render_bytes(render_content) {
            return (Some(blocks), None);
        }
    }
    if content.is_empty() {
        return (None, None);
    }
    let compiled = MessageRenderCompiler::compile(content);
    let serialized = MessageRenderCompiler::serialize(&compiled).ok();
    (serde_json::to_value(&compiled).ok(), serialized)
}

#[cfg(test)]
mod tests {
    use super::{resolve_render_blocks, topic_key};
    use crate::vcp_modules::message_repository::{MessageRenderCompiler, RENDERER_SCHEMA_VERSION};

    #[test]
    fn topic_identity_is_fail_closed() {
        assert!(topic_key("owner", "agent", "topic").is_ok());
        assert!(topic_key("", "agent", "topic").is_err());
        assert!(topic_key("owner", "user", "topic").is_err());
    }

    #[test]
    fn render_cache_requires_current_hash_schema_and_valid_payload() {
        let cached = MessageRenderCompiler::compile("cached body");
        let bytes = MessageRenderCompiler::serialize(&cached).expect("serialize cached blocks");
        let (valid, refresh) = resolve_render_blocks(
            "fresh body",
            "hash-a",
            Some(bytes.clone()),
            Some("hash-a"),
            Some(RENDERER_SCHEMA_VERSION),
        );
        assert!(serde_json::to_string(&valid)
            .unwrap()
            .contains("cached body"));
        assert!(refresh.is_none());

        for (payload, hash, schema) in [
            (
                Some(bytes.clone()),
                Some("stale-hash"),
                Some(RENDERER_SCHEMA_VERSION),
            ),
            (
                Some(bytes),
                Some("hash-a"),
                Some(RENDERER_SCHEMA_VERSION + 1),
            ),
            (
                Some(vec![0, 1, 2]),
                Some("hash-a"),
                Some(RENDERER_SCHEMA_VERSION),
            ),
        ] {
            let (resolved, refresh) =
                resolve_render_blocks("fresh body", "hash-a", payload, hash, schema);
            assert!(serde_json::to_string(&resolved)
                .unwrap()
                .contains("fresh body"));
            assert!(refresh.is_some());
        }
    }
}
