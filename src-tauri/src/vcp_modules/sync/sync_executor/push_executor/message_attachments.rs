use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};

struct AttachmentMetadata {
    size: u64,
    created_at: Option<u64>,
    image_frames: Option<Vec<String>>,
}

pub(super) async fn load_message_attachments(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    messages: &mut [ChatMessage],
) -> Result<Vec<ChatMessage>, String> {
    if messages.is_empty() {
        return Ok(messages.to_vec());
    }
    let placeholders = (0..messages.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT ma.msg_id, ma.hash AS relation_hash, ma.display_name, ma.src, ma.status,
                a.hash AS stored_hash, a.mime_type, a.size, a.internal_path,
                a.image_frames, a.thumbnail_path, a.created_at
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         WHERE ma.topic_id = ? AND ma.msg_id IN ({placeholders}) AND ma.deleted_at IS NULL
         ORDER BY ma.msg_id, ma.attachment_order ASC"
    );
    let mut query = sqlx::query(&query).bind(topic_id);
    for message in messages.iter() {
        query = query.bind(&message.id);
    }
    let rows = query
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Message attachment page query failed for {topic_id}: {error}"))?;
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut by_message = HashMap::new();
    for row in rows {
        let message_id: String = row
            .try_get("msg_id")
            .map_err(|error| format!("Attachment message id decode failed: {error}"))?;
        if !ids.contains(&message_id) {
            return Err(format!(
                "Attachment query returned unexpected message {topic_id}/{message_id}"
            ));
        }
        let attachment = decode_attachment_row(&row, topic_id, &message_id)?;
        by_message
            .entry(message_id)
            .or_insert_with(Vec::new)
            .push(attachment);
    }
    for message in messages.iter_mut() {
        if let Some(attachments) = by_message.remove(&message.id) {
            message.attachments = Some(attachments);
        }
    }
    Ok(messages.to_vec())
}

fn decode_attachment_row(
    row: &sqlx::sqlite::SqliteRow,
    topic_id: &str,
    message_id: &str,
) -> Result<Attachment, String> {
    let relation_hash = decode_relation_hash(row, topic_id, message_id)?;
    let metadata = decode_attachment_metadata(row, &relation_hash)?;
    let mime_type = row
        .try_get::<Option<String>, _>("mime_type")
        .map_err(|error| format!("Attachment MIME decode failed for {relation_hash}: {error}"))?
        .ok_or_else(|| format!("Attachment {relation_hash} has no MIME metadata"))?;
    let src = row
        .try_get::<Option<String>, _>("src")
        .map_err(|error| format!("Attachment source decode failed for {relation_hash}: {error}"))?
        .unwrap_or_default();
    let name = row
        .try_get("display_name")
        .map_err(|error| format!("Attachment name decode failed for {relation_hash}: {error}"))?;
    let status = row
        .try_get("status")
        .map_err(|error| format!("Attachment status decode failed: {error}"))?;
    let internal_path = row
        .try_get::<Option<String>, _>("internal_path")
        .map_err(|error| format!("Attachment path decode failed: {error}"))?
        .ok_or_else(|| "Attachment has no local path metadata".to_string())?;
    let thumbnail_path = row
        .try_get("thumbnail_path")
        .map_err(|error| format!("Attachment thumbnail decode failed: {error}"))?;
    Ok(Attachment {
        r#type: mime_type,
        src,
        name,
        size: metadata.size,
        hash: Some(relation_hash),
        status,
        internal_path,
        extracted_text: None,
        image_frames: metadata.image_frames,
        thumbnail_path,
        created_at: metadata.created_at,
    })
}

fn decode_relation_hash(
    row: &sqlx::sqlite::SqliteRow,
    topic_id: &str,
    message_id: &str,
) -> Result<String, String> {
    let relation_hash: String = row.try_get("relation_hash").map_err(|error| {
        format!("Attachment hash decode failed for {topic_id}/{message_id}: {error}")
    })?;
    let stored_hash: Option<String> = row.try_get("stored_hash").map_err(|error| {
        format!("Stored attachment hash decode failed for {relation_hash}: {error}")
    })?;
    if stored_hash.as_deref() != Some(relation_hash.as_str()) {
        return Err(format!(
            "Attachment {relation_hash} referenced by {topic_id}/{message_id} is missing locally"
        ));
    }
    Ok(relation_hash)
}

fn decode_attachment_metadata(
    row: &sqlx::sqlite::SqliteRow,
    relation_hash: &str,
) -> Result<AttachmentMetadata, String> {
    let size = row
        .try_get::<Option<i64>, _>("size")
        .map_err(|error| format!("Attachment size decode failed for {relation_hash}: {error}"))?
        .ok_or_else(|| format!("Attachment {relation_hash} has no local size metadata"))
        .and_then(|value| {
            u64::try_from(value)
                .map_err(|_| format!("Attachment {relation_hash} has a negative size"))
        })?;
    let created_at = row
        .try_get::<Option<i64>, _>("created_at")
        .map_err(|error| {
            format!("Attachment timestamp decode failed for {relation_hash}: {error}")
        })?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| format!("Attachment {relation_hash} has a negative timestamp"))
        })
        .transpose()?;
    let image_frames = row
        .try_get::<Option<String>, _>("image_frames")
        .map_err(|error| format!("Attachment frame decode failed for {relation_hash}: {error}"))?
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
                format!("Attachment {relation_hash} has invalid image frames: {error}")
            })
        })
        .transpose()?;
    Ok(AttachmentMetadata {
        size,
        created_at,
        image_frames,
    })
}
