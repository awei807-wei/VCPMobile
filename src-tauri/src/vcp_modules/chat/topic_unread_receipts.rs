use crate::vcp_modules::sync_hash::{HashAggregator, HashInitializer};
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Sqlite, Transaction};

/// Remove counted unread receipts and refresh the Wire configuration version
/// only when an Agent Topic's public `unread` flag actually flips to false.
pub(crate) async fn decrement_topic_unread_count(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    counted_unread: i64,
    updated_at: i64,
) -> Result<(), String> {
    if counted_unread <= 0 || !topic_has_unread_count(tx).await? {
        return Ok(());
    }
    let config_changed = if key.owner_type == "agent" {
        sqlx::query(
            "UPDATE topics
             SET unread_count = MAX(unread_count - ?, 0),
                 unread = 0,
                 updated_at = MAX(COALESCE(updated_at, 0), ?)
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL AND unread != 0
               AND MAX(unread_count - ?, 0) = 0",
        )
        .bind(counted_unread)
        .bind(updated_at)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(counted_unread)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?
        .rows_affected()
            == 1
    } else {
        false
    };
    if !config_changed {
        sqlx::query(
            "UPDATE topics
             SET unread_count = MAX(unread_count - ?, 0),
                 unread = CASE WHEN MAX(unread_count - ?, 0) = 0 THEN 0 ELSE unread END
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(counted_unread)
        .bind(counted_unread)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    }
    if config_changed {
        HashInitializer::recompute_topic_config_hash(tx, key).await?;
        HashAggregator::bubble_owner_from_topic_key(tx, key).await?;
    }
    Ok(())
}

async fn topic_has_unread_count(tx: &mut Transaction<'_, Sqlite>) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('topics') WHERE name = 'unread_count'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())
}
