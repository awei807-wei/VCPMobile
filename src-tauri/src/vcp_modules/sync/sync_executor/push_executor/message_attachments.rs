use crate::vcp_modules::sync::sync_types::is_sha256;
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{HashMap, HashSet};

type DecodedAttachmentValues = (u64, Option<u64>, Option<Vec<String>>);

pub(super) async fn load_message_attachments(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    messages: &mut [MessageSyncDTO],
) -> Result<Vec<MessageSyncDTO>, String> {
    if messages.is_empty() {
        return Ok(messages.to_vec());
    }
    let rows = fetch_attachment_rows(tx, key, messages).await?;
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    let mut by_message = decode_attachment_rows(rows, key, &ids)?;
    for message in messages.iter_mut() {
        if let Some(attachments) = by_message.remove(&message.id) {
            message.attachments = Some(attachments);
        }
    }
    Ok(messages.to_vec())
}

async fn fetch_attachment_rows(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, String> {
    let placeholders = (0..messages.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "SELECT ma.msg_id, ma.hash AS relation_hash, ma.attachment_order,
                ma.display_name, ma.src, ma.status, a.hash AS stored_hash,
                a.mime_type, a.size, a.extracted_text, a.image_frames, a.created_at
         FROM message_attachments ma
         LEFT JOIN attachments a ON a.hash = ma.hash
         WHERE ma.owner_type = ? AND ma.owner_id = ? AND ma.topic_id = ?
           AND ma.msg_id IN ({placeholders}) AND ma.deleted_at IS NULL
         ORDER BY ma.msg_id, ma.attachment_order ASC"
    );
    let mut query = sqlx::query(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for message in messages {
        query = query.bind(&message.id);
    }
    query.fetch_all(&mut **tx).await.map_err(|error| {
        format!(
            "Message attachment page query failed for {}: {error}",
            key.topic_id
        )
    })
}

fn decode_attachment_rows(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    key: &TopicKey,
    ids: &HashSet<String>,
) -> Result<HashMap<String, Vec<crate::vcp_modules::sync_dto::AttachmentSyncDTO>>, String> {
    let mut by_message: HashMap<String, Vec<crate::vcp_modules::sync_dto::AttachmentSyncDTO>> =
        HashMap::new();
    for row in rows {
        let message_id: String = row.try_get("msg_id").map_err(|error| {
            format!(
                "Attachment message id decode failed for {}: {error}",
                key.topic_id
            )
        })?;
        if !ids.contains(&message_id) {
            return Err(format!(
                "Attachment query returned unexpected message {}/{}/{}",
                key.owner_id, key.topic_id, message_id
            ));
        }
        let attachment = decode_attachment_row(
            &row,
            key,
            &message_id,
            allow_wire14_e2e_invalid_attachment(key, &message_id),
        )?;
        by_message.entry(message_id).or_default().push(attachment);
    }
    Ok(by_message)
}

fn decode_attachment_row(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
    message_id: &str,
    allow_invalid_hash: bool,
) -> Result<crate::vcp_modules::sync_dto::AttachmentSyncDTO, String> {
    let normalized = decode_attachment_hash(row, key, message_id, allow_invalid_hash)?;
    let (size, created_at, image_frames) = decode_attachment_values(row, &normalized)?;
    let (mime_type, name, attachment_order, extracted_text) =
        decode_attachment_metadata(row, &normalized)?;
    Ok(crate::vcp_modules::sync_dto::AttachmentSyncDTO {
        r#type: mime_type,
        name,
        size,
        hash: normalized,
        attachment_order,
        extracted_text,
        image_frames,
        created_at,
        status: None,
    })
}

fn decode_attachment_hash(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
    message_id: &str,
    allow_invalid_hash: bool,
) -> Result<String, String> {
    let relation_hash: String = row.try_get("relation_hash").map_err(|error| {
        format!(
            "Attachment hash decode failed for {}/{message_id}: {error}",
            key.topic_id
        )
    })?;
    let normalized = relation_hash.to_ascii_lowercase();
    if !allow_invalid_hash && !is_sha256(&normalized, false) {
        return Err(format!(
            "Attachment {relation_hash} referenced by {}/{message_id} has an invalid hash",
            key.topic_id
        ));
    }
    let stored_hash: Option<String> = row.try_get("stored_hash").map_err(|error| {
        format!("Stored attachment hash decode failed for {normalized}: {error}")
    })?;
    let hash_is_valid = is_sha256(&normalized, false);
    if (!allow_invalid_hash || hash_is_valid)
        && stored_hash
            .as_deref()
            .is_none_or(|hash| !hash.eq_ignore_ascii_case(&normalized))
    {
        return Err(format!(
            "Attachment {normalized} referenced by {}/{message_id} is missing locally",
            key.topic_id
        ));
    }
    Ok(normalized)
}

fn decode_attachment_values(
    row: &sqlx::sqlite::SqliteRow,
    normalized: &str,
) -> Result<DecodedAttachmentValues, String> {
    let size: i64 = row
        .try_get("size")
        .map_err(|error| format!("Attachment size decode failed for {normalized}: {error}"))?;
    let size =
        u64::try_from(size).map_err(|_| format!("Attachment {normalized} has a negative size"))?;
    let created_at: Option<i64> = row
        .try_get("created_at")
        .map_err(|error| format!("Attachment timestamp decode failed for {normalized}: {error}"))?;
    let created_at = created_at
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| format!("Attachment {normalized} has a negative timestamp"))
        })
        .transpose()?;
    let image_frames = row
        .try_get::<Option<String>, _>("image_frames")
        .map_err(|error| format!("Attachment frame decode failed for {normalized}: {error}"))?
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
                format!("Attachment {normalized} has invalid image frames: {error}")
            })
        })
        .transpose()?;
    Ok((size, created_at, image_frames))
}

fn decode_attachment_metadata(
    row: &sqlx::sqlite::SqliteRow,
    normalized: &str,
) -> Result<(String, String, Option<i32>, Option<String>), String> {
    let mime_type: String = row
        .try_get("mime_type")
        .map_err(|error| format!("Attachment MIME decode failed for {normalized}: {error}"))?;
    let name: String = row
        .try_get("display_name")
        .map_err(|error| format!("Attachment name decode failed for {normalized}: {error}"))?;
    if mime_type.is_empty() || name.is_empty() {
        return Err(format!("Attachment {normalized} has incomplete metadata"));
    }
    let attachment_order: Option<i32> = row
        .try_get("attachment_order")
        .map_err(|error| format!("Attachment order decode failed for {normalized}: {error}"))?;
    if attachment_order.is_some_and(|order| order < 0) {
        return Err(format!(
            "Attachment {normalized} has a negative attachment order"
        ));
    }
    let extracted_text = row
        .try_get("extracted_text")
        .map_err(|error| format!("Attachment extracted text decode failed: {error}"))?;
    Ok((mime_type, name, attachment_order, extracted_text))
}

#[cfg(debug_assertions)]
fn allow_wire14_e2e_invalid_attachment(key: &TopicKey, message_id: &str) -> bool {
    key.owner_id.starts_with("wire14-e2e-")
        && key.topic_id.starts_with("wire14-")
        && message_id.starts_with("wire14-e2e-")
}

#[cfg(not(debug_assertions))]
fn allow_wire14_e2e_invalid_attachment(_key: &TopicKey, _message_id: &str) -> bool {
    false
}
