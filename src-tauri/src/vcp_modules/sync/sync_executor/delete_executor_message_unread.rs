use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Sqlite, Transaction};

pub(super) async fn clear_message_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<(), String> {
    if ids.is_empty() || !message_unread_receipts_exist(tx).await? {
        return Ok(());
    }
    let counted_unread = count_counted_unread(tx, key, ids).await?;
    delete_message_unread_receipts(tx, key, ids).await?;
    if counted_unread > 0 {
        decrement_topic_unread(tx, key, counted_unread).await?;
    }
    Ok(())
}

async fn message_unread_receipts_exist(tx: &mut Transaction<'_, Sqlite>) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())
}

async fn count_counted_unread(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<i64, String> {
    let sql = format!(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1 AND msg_id IN ({})",
        super::placeholders(ids.len())
    );
    let mut query = sqlx::query_scalar::<_, i64>(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in ids {
        query = query.bind(id);
    }
    query
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

async fn delete_message_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    ids: &[String],
) -> Result<(), String> {
    let sql = format!(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({})",
        super::placeholders(ids.len())
    );
    let mut query = sqlx::query(&sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in ids {
        query = query.bind(id);
    }
    query
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn decrement_topic_unread(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    counted_unread: i64,
) -> Result<(), String> {
    if !topic_has_unread_count(tx).await? {
        return Ok(());
    }
    sqlx::query(
        "UPDATE topics
         SET unread_count = MAX(unread_count - ?, 0),
             unread = CASE WHEN MAX(unread_count - ?, 0) = 0 THEN 0 ELSE unread END,
             updated_at = MAX(COALESCE(updated_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(counted_unread)
    .bind(counted_unread)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
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
