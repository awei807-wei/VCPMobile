use super::{MessageRepository, PreparedUpsert};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::infra::file_manager::AttachmentReadGuard;
use crate::vcp_modules::topic_types::TopicKey;

pub(super) async fn persist_message_attachments(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    message: &ChatMessage,
    prepared: &PreparedUpsert,
    gate: &AttachmentReadGuard,
    roots: Option<&crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots>,
) -> Result<(), String> {
    if let Some(attachments) = &message.attachments {
        MessageRepository::upsert_attachments_for_message_with_roots(
            tx,
            key,
            &prepared.message_key.msg_id,
            prepared.message_timestamp,
            attachments,
            gate,
            roots,
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
