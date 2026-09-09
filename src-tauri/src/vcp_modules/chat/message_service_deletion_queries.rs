use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Sqlite, Transaction};

pub(super) fn placeholders(count: usize) -> String {
    (0..count).map(|_| "?").collect::<Vec<_>>().join(", ")
}

pub(super) async fn select_live_message_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<Vec<String>, String> {
    let query = format!(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({placeholders})
         ORDER BY timestamp ASC, msg_id ASC"
    );
    let mut request = sqlx::query_scalar::<_, String>(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        request = request.bind(msg_id);
    }
    request
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn select_active_message_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<Vec<String>, String> {
    let query = format!(
        "SELECT msg_id FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})
         ORDER BY msg_id ASC"
    );
    let mut request = sqlx::query_scalar::<_, String>(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        request = request.bind(msg_id);
    }
    request
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn mark_message_ids_deleted(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
    now: i64,
) -> Result<(), String> {
    let query = format!(
        "UPDATE messages SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND msg_id IN ({placeholders})"
    );
    let mut request = sqlx::query(&query)
        .bind(now)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        request = request.bind(msg_id);
    }
    request
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) async fn delete_message_related_rows(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<(), String> {
    for table in [
        "render_cache",
        "message_attachments",
        "active_generations",
        "messages_fts",
    ] {
        let query = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({placeholders})"
        );
        let mut request = sqlx::query(&query)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id);
        for msg_id in msg_ids {
            request = request.bind(msg_id);
        }
        request
            .execute(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    }
    delete_message_unread_receipts(tx, key, msg_ids, placeholders).await?;
    Ok(())
}

pub(super) async fn delete_message_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    let count_sql = format!(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1 AND msg_id IN ({placeholders})"
    );
    let mut count_request = sqlx::query_scalar::<_, i64>(&count_sql)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        count_request = count_request.bind(msg_id);
    }
    let counted_unread = count_request
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    let query = format!(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})"
    );
    let mut request = sqlx::query(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for msg_id in msg_ids {
        request = request.bind(msg_id);
    }
    request
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    decrement_topic_unread_count(tx, key, counted_unread).await
}

pub(super) async fn decrement_topic_unread_count(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    counted_unread: i64,
) -> Result<(), String> {
    if counted_unread <= 0 {
        return Ok(());
    }
    let has_unread_count: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('topics') WHERE name = 'unread_count'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if !has_unread_count {
        return Ok(());
    }
    let now = crate::vcp_modules::infra::utils::now_millis();
    sqlx::query(
        "UPDATE topics
         SET unread_count = MAX(unread_count - ?, 0),
             unread = CASE WHEN MAX(unread_count - ?, 0) = 0 THEN 0 ELSE unread END,
             updated_at = MAX(COALESCE(updated_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(counted_unread)
    .bind(counted_unread)
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}
