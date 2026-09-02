use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use rusqlite::ToSql;

pub(super) fn write_attachments(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
) -> rusqlite::Result<()> {
    let relations = collect_attachment_relations(tx, messages)?;
    let message_ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    delete_message_attachments(tx, topic_id, &message_ids)?;
    insert_attachment_relations(tx, topic_id, &relations)
}

#[derive(Debug)]
struct AttachmentRelation {
    message_id: String,
    hash: String,
    order: i32,
    display_name: String,
    src: String,
    status: String,
    created_at: i64,
}

fn collect_attachment_relations(
    tx: &rusqlite::Transaction<'_>,
    messages: &[ChatMessage],
) -> rusqlite::Result<Vec<AttachmentRelation>> {
    let mut relations = Vec::new();
    for message in messages {
        let Some(attachments) = &message.attachments else {
            continue;
        };
        for (order, attachment) in attachments.iter().enumerate() {
            let hash = attachment.hash.clone().unwrap_or_else(|| {
                crate::vcp_modules::infra::utils::calculate_sha256(attachment.src.as_bytes())
            });
            upsert_attachment_core(tx, &hash, attachment, message.timestamp as i64)?;
            relations.push(AttachmentRelation {
                message_id: message.id.clone(),
                hash,
                order: order as i32,
                display_name: attachment.name.clone(),
                src: attachment.src.clone(),
                status: attachment
                    .status
                    .clone()
                    .unwrap_or_else(|| "ready".to_string()),
                created_at: message.timestamp as i64,
            });
        }
    }
    Ok(relations)
}

fn delete_message_attachments(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for chunk in message_ids.chunks(999) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "DELETE FROM message_attachments
             WHERE topic_id = ? AND msg_id IN ({placeholders})"
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + 1);
        params.push(Box::new(topic_id.to_string()));
        for id in chunk {
            params.push(Box::new(id.clone()));
        }
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
    topic_id: &str,
    relations: &[AttachmentRelation],
) -> rusqlite::Result<()> {
    const PARAMS_PER_RELATION: usize = 8;
    for chunk in relations.chunks(999 / PARAMS_PER_RELATION) {
        let values = vec!["(?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO message_attachments
             (topic_id, msg_id, hash, attachment_order, display_name, src, status, created_at)
             VALUES {values}"
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() * 8);
        for relation in chunk {
            params.extend([
                Box::new(topic_id.to_string()) as Box<dyn ToSql>,
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
            internal_path = excluded.internal_path, extracted_text = excluded.extracted_text,
            image_frames = excluded.image_frames, thumbnail_path = excluded.thumbnail_path,
            updated_at = excluded.updated_at",
        rusqlite::params![
            hash,
            &attachment.r#type,
            attachment.size as i64,
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
