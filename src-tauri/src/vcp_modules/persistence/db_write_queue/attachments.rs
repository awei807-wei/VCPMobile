use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::sync_dto::{AttachmentSyncDTO, MessageSyncDTO};
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::ToSql;

#[derive(Debug)]
struct AttachmentRelation {
    message_id: String,
    hash: String,
    order: i32,
    display_name: String,
    src: Option<String>,
    status: Option<String>,
    created_at: i64,
}

/// Persist local attachment metadata and a composite message relation.
///
/// This compatibility path retains local `src`/`status` in the local index;
/// those fields never pass through `MessageSyncDTO`.
pub(super) fn write_attachments(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[ChatMessage],
    canonical_messages: &[MessageSyncDTO],
) -> rusqlite::Result<()> {
    if messages.len() != canonical_messages.len() {
        return Err(
            crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(
                "Message and canonical DTO attachment batches have inconsistent lengths",
            ),
        );
    }
    let mut relations = Vec::new();
    for (message, canonical) in messages.iter().zip(canonical_messages) {
        let canonical_attachments = canonical.attachments.as_deref().unwrap_or_default();
        if let Some(attachments) = &message.attachments {
            if attachments.len() != canonical_attachments.len() {
                return Err(
                    crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(format!(
                        "Message {} local/canonical attachment counts differ",
                        message.id
                    )),
                );
            }
            for (index, (attachment, dto)) in
                attachments.iter().zip(canonical_attachments).enumerate()
            {
                let created_at = attachment
                    .created_at
                    .unwrap_or(message.timestamp)
                    .try_into()
                    .map_err(|_| {
                        crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(
                            format!("Attachment {} timestamp is too large", attachment.name),
                        )
                    })?;
                upsert_attachment_core(tx, &dto.hash, attachment, created_at)?;
                relations.push(AttachmentRelation {
                    message_id: message.id.clone(),
                    hash: dto.hash.clone(),
                    order: attachment_order(dto, attachment, index)?,
                    display_name: attachment.name.clone(),
                    src: Some(attachment.src.clone()),
                    status: attachment.status.clone(),
                    created_at,
                });
            }
        }
    }
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    delete_message_attachments(tx, key, &message_ids)?;
    insert_attachment_relations(tx, key, &relations)
}

/// Persist a canonical Wire 1.4 attachment relation. Local path/status stay
/// NULL so a peer cannot manufacture local filesystem state.
pub(super) fn write_attachments_for_dto(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
) -> rusqlite::Result<()> {
    let mut relations = Vec::new();
    for message in messages {
        let Some(attachments) = &message.attachments else {
            continue;
        };
        for (index, attachment) in attachments.iter().enumerate() {
            let created_at = attachment
                .created_at
                .unwrap_or(message.timestamp)
                .try_into()
                .map_err(|_| {
                    crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(format!(
                        "Attachment {} timestamp is too large",
                        attachment.name
                    ))
                })?;
            upsert_attachment_core_for_dto(tx, attachment, created_at)?;
            relations.push(AttachmentRelation {
                message_id: message.id.clone(),
                hash: attachment.hash.clone(),
                order: attachment_order_for_dto(attachment, index)?,
                display_name: attachment.name.clone(),
                src: None,
                status: None,
                created_at,
            });
        }
    }
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    delete_message_attachments(tx, key, &message_ids)?;
    insert_attachment_relations(tx, key, &relations)
}

fn attachment_order(
    dto: &AttachmentSyncDTO,
    attachment: &Attachment,
    index: usize,
) -> rusqlite::Result<i32> {
    let order = dto
        .attachment_order
        .or(attachment.attachment_order)
        .unwrap_or(index as i32);
    if order < 0 {
        return Err(
            crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(format!(
                "Attachment {} has a negative attachment order",
                attachment.name
            )),
        );
    }
    Ok(order)
}

fn attachment_order_for_dto(attachment: &AttachmentSyncDTO, index: usize) -> rusqlite::Result<i32> {
    let order = attachment.attachment_order.unwrap_or(index as i32);
    if order < 0 {
        return Err(
            crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(format!(
                "Attachment {} has a negative attachment order",
                attachment.name
            )),
        );
    }
    Ok(order)
}

fn delete_message_attachments(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for chunk in message_ids.chunks(998) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "DELETE FROM message_attachments
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({placeholders})"
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + 3);
        params.extend([
            Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
            Box::new(key.owner_id.clone()),
            Box::new(key.topic_id.clone()),
        ]);
        params.extend(
            chunk
                .iter()
                .cloned()
                .map(|id| Box::new(id) as Box<dyn ToSql>),
        );
        let refs = params
            .iter()
            .map(|param| param.as_ref())
            .collect::<Vec<_>>();
        tx.execute(&sql, &*refs)?;
    }
    Ok(())
}

fn insert_attachment_relations(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    relations: &[AttachmentRelation],
) -> rusqlite::Result<()> {
    const PARAMS_PER_RELATION: usize = 10;
    for chunk in relations.chunks(999 / PARAMS_PER_RELATION) {
        if chunk.is_empty() {
            continue;
        }
        let values = vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO message_attachments
             (owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
              display_name, src, status, created_at)
             VALUES {values}"
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() * PARAMS_PER_RELATION);
        for relation in chunk {
            params.extend([
                Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
                Box::new(key.owner_id.clone()),
                Box::new(key.topic_id.clone()),
                Box::new(relation.message_id.clone()),
                Box::new(relation.hash.clone()),
                Box::new(relation.order),
                Box::new(relation.display_name.clone()),
                Box::new(relation.src.clone()),
                Box::new(relation.status.clone()),
                Box::new(relation.created_at),
            ]);
        }
        let refs = params
            .iter()
            .map(|param| param.as_ref())
            .collect::<Vec<_>>();
        tx.execute(&sql, &*refs)?;
    }
    Ok(())
}

fn upsert_attachment_core(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    attachment: &Attachment,
    timestamp: i64,
) -> rusqlite::Result<()> {
    let image_frames = attachment
        .image_frames
        .as_ref()
        .and_then(|frames| serde_json::to_string(frames).ok());
    tx.execute(
        "INSERT INTO attachments (
            hash, mime_type, size, internal_path, extracted_text, image_frames,
            thumbnail_path, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
            mime_type = excluded.mime_type, size = excluded.size,
            internal_path = CASE
                WHEN excluded.internal_path <> '' THEN excluded.internal_path
                ELSE attachments.internal_path
            END,
            extracted_text = COALESCE(attachments.extracted_text, excluded.extracted_text),
            image_frames = COALESCE(attachments.image_frames, excluded.image_frames),
            thumbnail_path = COALESCE(attachments.thumbnail_path, excluded.thumbnail_path),
            updated_at = excluded.updated_at",
        rusqlite::params![
            hash,
            &attachment.r#type,
            i64::try_from(attachment.size).unwrap_or(i64::MAX),
            &attachment.internal_path,
            &attachment.extracted_text,
            image_frames,
            &attachment.thumbnail_path,
            timestamp,
            timestamp
        ],
    )?;
    Ok(())
}

fn upsert_attachment_core_for_dto(
    tx: &rusqlite::Transaction<'_>,
    attachment: &AttachmentSyncDTO,
    timestamp: i64,
) -> rusqlite::Result<()> {
    let image_frames = attachment
        .image_frames
        .as_ref()
        .and_then(|frames| serde_json::to_string(frames).ok());
    tx.execute(
        "INSERT INTO attachments (
            hash, mime_type, size, internal_path, extracted_text, image_frames,
            thumbnail_path, created_at, updated_at
        ) VALUES (?, ?, ?, '', ?, ?, NULL, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
            mime_type = excluded.mime_type, size = excluded.size,
            extracted_text = COALESCE(attachments.extracted_text, excluded.extracted_text),
            image_frames = COALESCE(attachments.image_frames, excluded.image_frames),
            updated_at = excluded.updated_at",
        rusqlite::params![
            &attachment.hash,
            &attachment.r#type,
            i64::try_from(attachment.size).unwrap_or(i64::MAX),
            &attachment.extracted_text,
            image_frames,
            timestamp,
            timestamp
        ],
    )?;
    Ok(())
}
