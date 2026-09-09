use super::ExistingMessageState;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::Row;

pub(super) fn decode_existing_message(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ExistingMessageState, String> {
    Ok(ExistingMessageState {
        role: row.try_get("role").map_err(|error| error.to_string())?,
        name: row.try_get("name").map_err(|error| error.to_string())?,
        agent_id: row.try_get("agent_id").map_err(|error| error.to_string())?,
        content: decode_message_content(row, "content")?,
        timestamp: row
            .try_get("timestamp")
            .map_err(|error| error.to_string())?,
        is_group_message: row
            .try_get::<i64, _>("is_group_message")
            .map_err(|error| error.to_string())?
            != 0,
        group_id: row.try_get("group_id").map_err(|error| error.to_string())?,
        finish_reason: row
            .try_get("finish_reason")
            .map_err(|error| error.to_string())?,
        content_hash: row
            .try_get("content_hash")
            .map_err(|error| error.to_string())?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|error| error.to_string())?,
        deleted_at: row
            .try_get("deleted_at")
            .map_err(|error| error.to_string())?,
    })
}
