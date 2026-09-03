use super::registry;
use crate::vcp_modules::chat::topic_types::MessageKey;

#[path = "vcp_client_recovery_support.rs"]
mod support;

#[path = "vcp_client_recovery_flow.rs"]
mod flow;

pub use flow::recover_active_generation;

pub(super) async fn resolve_legacy_generation_key(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    message_id: &str,
) -> Result<MessageKey, String> {
    use sqlx::Row;

    let active_rows = sqlx::query(
        "SELECT owner_type, owner_id, topic_id, msg_id
         FROM active_generations WHERE msg_id = ?",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;

    if active_rows.len() > 1 {
        return Err(format!(
            "Cannot recover legacy messageId {message_id}: active generation is ambiguous"
        ));
    }
    if let Some(row) = active_rows.first() {
        return registry::message_key_from_parts(
            row.get("owner_id"),
            row.get("owner_type"),
            row.get("topic_id"),
            row.get("msg_id"),
        );
    }

    let message_rows = sqlx::query(
        "SELECT owner_type, owner_id, topic_id, msg_id
         FROM messages WHERE msg_id = ? AND deleted_at IS NULL",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;
    if message_rows.len() != 1 {
        return Err(if message_rows.is_empty() {
            format!("Message {message_id} not found for recovery")
        } else {
            format!("Cannot recover legacy messageId {message_id}: message identity is ambiguous")
        });
    }
    let row = &message_rows[0];
    registry::message_key_from_parts(
        row.get("owner_id"),
        row.get("owner_type"),
        row.get("topic_id"),
        row.get("msg_id"),
    )
}

async fn legacy_recovery_file_is_unambiguous(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<bool, String> {
    let identities: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE msg_id = ? AND deleted_at IS NULL")
            .bind(&key.msg_id)
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())?;
    Ok(identities == 1)
}
