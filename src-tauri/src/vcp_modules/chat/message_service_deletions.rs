use super::message_service_support::{resolve_unique_topic_key, topic_key};
use crate::vcp_modules::topic_types::TopicKey;
use tauri::{AppHandle, Manager};

/// Compatibility facade retained for callers that only have a topic id.
/// Duplicate topic ids across owners fail closed in the resolver.
pub async fn delete_messages(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_id: &str,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    let key = resolve_unique_topic_key(db_pool, topic_id).await?;
    delete_messages_for_topic(db_pool, &key, msg_ids).await
}

/// Owner-aware logical deletion. The tombstone clock is monotonic and every
/// secondary table is constrained by the same composite topic/message key.
pub async fn delete_messages_for_topic(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    msg_ids: Vec<String>,
) -> Result<(), String> {
    if !key.is_valid() {
        return Err("invalid topic identity".to_string());
    }
    if msg_ids.is_empty() {
        return Ok(());
    }
    let mut tx = db_pool.begin().await.map_err(|error| error.to_string())?;
    let placeholders = msg_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let now = chrono::Utc::now().timestamp_millis();
    mark_messages_deleted(&mut tx, key, &msg_ids, placeholders.as_str(), now).await?;
    delete_message_related_rows(&mut tx, key, &msg_ids, placeholders.as_str()).await?;
    delete_message_fts_rows(&mut tx, key, &msg_ids, placeholders.as_str()).await?;
    refresh_topic_message_count(&mut tx, key, now).await?;
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic_for_key(&mut tx, key).await?;
    tx.commit().await.map_err(|error| error.to_string())
}

pub async fn truncate_history_after_timestamp(
    _app_handle: AppHandle,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    timestamp: i64,
) -> Result<(), String> {
    let key = topic_key(owner_id, owner_type, topic_id)?;
    truncate_history_after_timestamp_for_topic(db_pool, &key, timestamp).await
}

/// Owner-aware truncation for regeneration and history replay.
pub async fn truncate_history_after_timestamp_for_topic(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &TopicKey,
    timestamp: i64,
) -> Result<(), String> {
    if !key.is_valid() {
        return Err("invalid topic identity".to_string());
    }
    let mut tx = db_pool.begin().await.map_err(|error| error.to_string())?;
    let now = chrono::Utc::now().timestamp_millis();
    delete_rows_after_timestamp(
        &mut tx,
        key,
        timestamp,
        ["render_cache", "message_attachments"],
    )
    .await?;
    mark_rows_deleted_after_timestamp(&mut tx, key, timestamp, now).await?;
    delete_rows_after_timestamp(
        &mut tx,
        key,
        timestamp,
        ["active_generations", "messages_fts"],
    )
    .await?;
    refresh_topic_message_count(&mut tx, key, now).await?;
    tx.commit().await.map_err(|error| error.to_string())
}

async fn mark_messages_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
    now: i64,
) -> Result<(), String> {
    let query_string = format!(
        "UPDATE messages SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})"
    );
    let mut query = sqlx::query(&query_string)
        .bind(now)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in msg_ids {
        query = query.bind(id);
    }
    query
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn delete_message_related_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<(), String> {
    for table in ["render_cache", "message_attachments", "active_generations"] {
        let query_string = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({placeholders})"
        );
        execute_message_id_query(tx, &query_string, key, msg_ids).await?;
    }
    Ok(())
}

async fn delete_message_fts_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_ids: &[String],
    placeholders: &str,
) -> Result<(), String> {
    let query_string = format!(
        "DELETE FROM messages_fts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})"
    );
    execute_message_id_query(tx, &query_string, key, msg_ids).await
}

async fn execute_message_id_query(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    query_string: &str,
    key: &TopicKey,
    msg_ids: &[String],
) -> Result<(), String> {
    let mut query = sqlx::query(query_string)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id);
    for id in msg_ids {
        query = query.bind(id);
    }
    query
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn refresh_topic_message_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    now: i64,
) -> Result<(), String> {
    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE topics
         SET msg_count = ?, updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(msg_count)
    .bind(now)
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn delete_rows_after_timestamp<const N: usize>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    timestamp: i64,
    tables: [&str; N],
) -> Result<(), String> {
    let select_ids = "
        SELECT msg_id FROM messages
        WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND timestamp > ?
    ";
    for table in tables {
        let query_string = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN ({select_ids})"
        );
        sqlx::query(&query_string)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(timestamp)
            .execute(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn mark_rows_deleted_after_timestamp(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    timestamp: i64,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE messages
         SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND timestamp > ?",
    )
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(timestamp)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Removes an attachment relation using the complete message identity.
#[tauri::command]
pub async fn delete_message_attachment(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
    hash: String,
) -> Result<(), String> {
    delete_message_attachment_for_key(
        &app_handle,
        &owner_type,
        &owner_id,
        &topic_id,
        &message_id,
        &hash,
    )
    .await
}

/// Owner-aware attachment tombstone and topic hash bubble.
pub async fn delete_message_attachment_for_key(
    app_handle: &tauri::AppHandle,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    message_id: &str,
    hash: &str,
) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let key = topic_key(owner_id, owner_type, topic_id)?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    sqlx::query(
        "UPDATE message_attachments
         SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id = ? AND hash = ?",
    )
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(hash)
    .execute(&db_state.pool)
    .await
    .map_err(|error| error.to_string())?;
    let mut tx = db_state
        .pool
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic_for_key(&mut tx, &key).await?;
    tx.commit().await.map_err(|error| error.to_string())
}
