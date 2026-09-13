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
        crate::vcp_modules::chat::topic_unread_receipts::decrement_topic_unread_count(
            tx,
            key,
            counted_unread,
            crate::vcp_modules::infra::utils::now_millis(),
        )
        .await?;
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
