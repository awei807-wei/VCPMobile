use super::deletion_queries::decrement_topic_unread_count;
use super::MessageMutationAnchor;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Sqlite, Transaction};

pub(super) async fn truncate_history_after_timestamp_for_topic(
    db_pool: &sqlx::Pool<Sqlite>,
    key: &TopicKey,
    anchor_message_id: &str,
    include_anchor: bool,
) -> Result<super::MessageMutationResult, String> {
    if !key.is_valid() || anchor_message_id.is_empty() {
        return Err("截断历史需要完整话题身份和 anchorMessageId".to_string());
    }
    let mut tx = db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|error| error.to_string())?;
    let (anchor_timestamp, mut anchor) = load_anchor(&mut tx, key, anchor_message_id).await?;
    anchor.include_anchor = include_anchor;
    let predicate = order_predicate(include_anchor);
    let qualified_predicate = qualified_order_predicate(include_anchor);
    let (deleted_ids, active_ids) = select_ordered_ids(
        &mut tx,
        key,
        anchor_timestamp,
        anchor_message_id,
        qualified_predicate,
    )
    .await?;
    delete_ordered_related_rows(
        &mut tx,
        key,
        anchor_timestamp,
        anchor_message_id,
        qualified_predicate,
    )
    .await?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    mark_ordered_messages_deleted(
        &mut tx,
        key,
        anchor_timestamp,
        anchor_message_id,
        predicate,
        now,
        deleted_ids.len(),
    )
    .await?;
    let msg_count = refresh_topic_message_count(&mut tx, key, now).await?;
    bubble_topic_hash(&mut tx, key).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(super::MessageMutationResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count,
        anchor: Some(anchor),
    })
}

async fn select_ordered_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let deleted_ids =
        select_ordered_message_ids(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    let active_ids =
        select_ordered_active_ids(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    Ok((deleted_ids, active_ids))
}

async fn load_anchor(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_message_id: &str,
) -> Result<(i64, MessageMutationAnchor), String> {
    let row = sqlx::query(
        "SELECT timestamp FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_message_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("anchorMessageId {anchor_message_id} 不存在、已删除或不属于当前话题"))?;
    let timestamp: i64 =
        sqlx::Row::try_get(&row, "timestamp").map_err(|error| error.to_string())?;
    Ok((
        timestamp,
        MessageMutationAnchor {
            message_id: anchor_message_id.to_string(),
            timestamp,
            include_anchor: false,
        },
    ))
}

pub(crate) fn order_predicate(include_anchor: bool) -> &'static str {
    if include_anchor {
        "(timestamp > ? OR (timestamp = ? AND msg_id >= ?))"
    } else {
        "(timestamp > ? OR (timestamp = ? AND msg_id > ?))"
    }
}

pub(crate) fn qualified_order_predicate(include_anchor: bool) -> &'static str {
    if include_anchor {
        "(m.timestamp > ? OR (m.timestamp = ? AND m.msg_id >= ?))"
    } else {
        "(m.timestamp > ? OR (m.timestamp = ? AND m.msg_id > ?))"
    }
}

pub(crate) async fn select_ordered_message_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<Vec<String>, String> {
    let query = format!(
        "SELECT m.msg_id FROM messages m
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
           AND m.deleted_at IS NULL AND {predicate}
         ORDER BY m.timestamp ASC, m.msg_id ASC"
    );
    sqlx::query_scalar::<_, String>(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_timestamp)
        .bind(anchor_timestamp)
        .bind(anchor_message_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn select_ordered_active_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<Vec<String>, String> {
    let query = format!(
        "SELECT a.msg_id FROM active_generations a
         JOIN messages m
           ON m.owner_type = a.owner_type AND m.owner_id = a.owner_id
          AND m.topic_id = a.topic_id AND m.msg_id = a.msg_id
         WHERE a.owner_type = ? AND a.owner_id = ? AND a.topic_id = ?
           AND {predicate}
         ORDER BY m.timestamp ASC, m.msg_id ASC"
    );
    sqlx::query_scalar::<_, String>(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_timestamp)
        .bind(anchor_timestamp)
        .bind(anchor_message_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn delete_ordered_related_rows(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(), String> {
    for table in [
        "render_cache",
        "message_attachments",
        "messages_fts",
        "active_generations",
    ] {
        let query = format!(
            "DELETE FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id IN (
                 SELECT m.msg_id FROM messages m
                 WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
                   AND {predicate}
               )"
        );
        sqlx::query(&query)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(&key.owner_type)
            .bind(&key.owner_id)
            .bind(&key.topic_id)
            .bind(anchor_timestamp)
            .bind(anchor_timestamp)
            .bind(anchor_message_id)
            .execute(&mut **tx)
            .await
            .map_err(|error| error.to_string())?;
    }
    delete_ordered_unread_receipts(tx, key, anchor_timestamp, anchor_message_id, predicate).await
}

async fn delete_ordered_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(), String> {
    if !has_unread_receipts_table(tx).await? {
        return Ok(());
    }
    let counted_unread =
        count_ordered_unread_receipts(tx, key, anchor_timestamp, anchor_message_id, predicate)
            .await?;
    delete_ordered_unread_receipt_rows(tx, key, anchor_timestamp, anchor_message_id, predicate)
        .await?;
    decrement_topic_unread_count(tx, key, counted_unread).await
}

async fn has_unread_receipts_table(tx: &mut Transaction<'_, Sqlite>) -> Result<bool, String> {
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

async fn count_ordered_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<i64, String> {
    let query = format!(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1 AND msg_id IN (
             SELECT m.msg_id FROM messages m
             WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
               AND {predicate}
           )"
    );
    sqlx::query_scalar(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_timestamp)
        .bind(anchor_timestamp)
        .bind(anchor_message_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| error.to_string())
}

async fn delete_ordered_unread_receipt_rows(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(), String> {
    let query = format!(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN (
             SELECT m.msg_id FROM messages m
             WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
               AND {predicate}
           )"
    );
    sqlx::query(&query)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_timestamp)
        .bind(anchor_timestamp)
        .bind(anchor_message_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) async fn mark_ordered_messages_deleted(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
    now: i64,
    expected_count: usize,
) -> Result<(), String> {
    let query = format!(
        "UPDATE messages SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL AND {predicate}"
    );
    let changed = sqlx::query(&query)
        .bind(now)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(anchor_timestamp)
        .bind(anchor_timestamp)
        .bind(anchor_message_id)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    if changed.rows_affected() != expected_count as u64 {
        return Err(format!(
            "稳定锚点截断行数不一致：实际 {}，预期 {}",
            changed.rows_affected(),
            expected_count
        ));
    }
    Ok(())
}

pub(crate) async fn refresh_topic_message_count(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    now: i64,
) -> Result<i32, String> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    let msg_count = i32::try_from(count).map_err(|_| "消息数量超出整数范围".to_string())?;
    let updated = sqlx::query(
        "UPDATE topics
         SET msg_count = ?, updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(msg_count)
    .bind(now)
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if updated.rows_affected() != 1 {
        return Err(format!("话题 {} 不存在或已删除", key.topic_id));
    }
    Ok(msg_count)
}

pub(crate) async fn bubble_topic_hash(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
) -> Result<(), String> {
    crate::vcp_modules::sync_hash::HashAggregator::bubble_from_topic_for_key(tx, key).await
}
