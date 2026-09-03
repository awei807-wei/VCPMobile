use super::MessageRepository;
use crate::vcp_modules::chat_manager::Attachment;
use crate::vcp_modules::topic_types::TopicKey;

impl MessageRepository {
    pub(crate) async fn upsert_attachments_for_message(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        key: &TopicKey,
        msg_id: &str,
        timestamp: i64,
        attachments: &[Attachment],
    ) -> Result<(), String> {
        delete_attachment_relations(tx, key, msg_id).await?;
        for (index, attachment) in ordered_attachments(attachments, key, msg_id)? {
            upsert_attachment(tx, key, msg_id, timestamp, index, attachment).await?;
        }
        Ok(())
    }
}

async fn delete_attachment_relations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn ordered_attachments<'a>(
    attachments: &'a [Attachment],
    key: &TopicKey,
    msg_id: &str,
) -> Result<Vec<(i32, &'a Attachment)>, String> {
    let mut ordered = attachments.iter().enumerate().collect::<Vec<_>>();
    ordered.sort_by_key(|(index, attachment)| {
        (
            attachment.attachment_order.unwrap_or(*index as i32),
            *index as i32,
        )
    });
    let mut seen_orders = std::collections::HashSet::with_capacity(ordered.len());
    ordered
        .into_iter()
        .map(|(index, attachment)| {
            let order = attachment.attachment_order.unwrap_or(index as i32);
            if order < 0 {
                return Err(format!(
                    "attachment {} has a negative attachment order",
                    attachment.name
                ));
            }
            if !seen_orders.insert(order) {
                return Err(format!(
                    "message {}/{} contains duplicate attachment order {}",
                    key.topic_id, msg_id, order
                ));
            }
            Ok((order, attachment))
        })
        .collect()
}

async fn upsert_attachment(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    timestamp: i64,
    attachment_order: i32,
    attachment: &Attachment,
) -> Result<(), String> {
    let hash = attachment.hash.clone().unwrap_or_else(|| {
        crate::vcp_modules::infra::utils::calculate_sha256(attachment.src.as_bytes())
    });
    let image_frames = attachment
        .image_frames
        .as_ref()
        .and_then(|frames| serde_json::to_string(frames).ok());
    sqlx::query(
        "INSERT INTO attachments (
            hash, mime_type, size, internal_path, extracted_text, image_frames,
            thumbnail_path, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
            mime_type = excluded.mime_type, size = excluded.size,
            internal_path = CASE WHEN excluded.internal_path <> ''
                THEN excluded.internal_path ELSE attachments.internal_path END,
            extracted_text = COALESCE(attachments.extracted_text, excluded.extracted_text),
            image_frames = COALESCE(attachments.image_frames, excluded.image_frames),
            thumbnail_path = COALESCE(attachments.thumbnail_path, excluded.thumbnail_path),
            updated_at = excluded.updated_at",
    )
    .bind(&hash)
    .bind(&attachment.r#type)
    .bind(i64::try_from(attachment.size).unwrap_or(i64::MAX))
    .bind(&attachment.internal_path)
    .bind(&attachment.extracted_text)
    .bind(image_frames)
    .bind(&attachment.thumbnail_path)
    .bind(timestamp)
    .bind(timestamp)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    insert_attachment_relation(
        tx,
        key,
        msg_id,
        timestamp,
        attachment_order,
        attachment,
        &hash,
    )
    .await
}

async fn insert_attachment_relation(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    timestamp: i64,
    attachment_order: i32,
    attachment: &Attachment,
    hash: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO message_attachments (
            owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
            display_name, src, status, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .bind(hash)
    .bind(attachment_order)
    .bind(&attachment.name)
    .bind(&attachment.src)
    .bind(&attachment.status)
    .bind(timestamp)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}
