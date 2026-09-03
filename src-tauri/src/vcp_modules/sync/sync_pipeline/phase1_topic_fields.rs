use crate::vcp_modules::sync_types::OwnerType;
use sqlx::Row;

pub(super) fn decode_topic_live_fields(
    row: &sqlx::sqlite::SqliteRow,
    owner_type: &OwnerType,
    owner_id: &str,
    topic_id: &str,
) -> Result<(String, String, i64), String> {
    let config_hash: String = row.try_get("config_hash").map_err(|error| {
        format!(
            "Topic manifest config hash decode failed for {owner_type}/{owner_id}/{topic_id}: {error}"
        )
    })?;
    let content_hash: String = row.try_get("content_hash").map_err(|error| {
        format!(
            "Topic manifest content hash decode failed for {owner_type}/{owner_id}/{topic_id}: {error}"
        )
    })?;
    let updated_at = row.try_get("updated_at").map_err(|error| {
        format!(
            "Topic manifest timestamp decode failed for {owner_type}/{owner_id}/{topic_id}: {error}"
        )
    })?;
    Ok((
        super::required_hash(
            config_hash,
            &format!("Topic {owner_type} configHash"),
            false,
        )?,
        super::required_hash(
            content_hash,
            &format!("Topic {owner_type} contentHash"),
            true,
        )?,
        super::required_timestamp(updated_at, &format!("Topic {owner_type} updatedAt"))?,
    ))
}
