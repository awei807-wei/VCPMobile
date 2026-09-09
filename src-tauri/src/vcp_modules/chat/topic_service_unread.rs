use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use serde::Serialize;
use sqlx::{Row, Sqlite, Transaction};
use tauri::{AppHandle, State};

/// 后端返回的未读状态，前端只接受这份事务提交前读取的权威结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicUnreadState {
    pub owner_id: String,
    pub owner_type: String,
    pub topic_id: String,
    pub unread: bool,
    pub unread_count: i32,
    /// Same-transaction authoritative aggregate for the owner list.
    pub owner_unread_count: i32,
}

/// 单个 Agent/Group 的权威未读聚合。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerUnreadState {
    pub owner_id: String,
    pub owner_type: String,
    pub unread_count: i32,
}

#[tauri::command]
pub async fn set_topic_unread(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    unread: bool,
) -> Result<TopicUnreadState, String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    set_topic_unread_in_pool(
        &db_state.pool,
        &topic_key,
        unread,
        crate::vcp_modules::infra::utils::now_millis(),
    )
    .await
}

/// 查询单个 owner 的权威未读聚合，避免刷新整个 Agent/Group 列表。
#[tauri::command]
pub async fn get_owner_unread_count(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
) -> Result<OwnerUnreadState, String> {
    let owner_key = TopicKey::new(owner_type, owner_id, "owner-unread-query");
    validate_owner_key(&owner_key)?;
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    let unread_count = load_owner_unread_count(&mut tx, &owner_key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(OwnerUnreadState {
        owner_id: owner_key.owner_id,
        owner_type: owner_key.owner_type,
        unread_count,
    })
}

pub(crate) async fn set_topic_unread_in_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_key: &TopicKey,
    unread: bool,
    updated_at: i64,
) -> Result<TopicUnreadState, String> {
    validate_topic_key(topic_key)?;
    let unread_int = i32::from(unread);
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let changed = sqlx::query(
        "UPDATE topics
         SET unread = ?,
             unread_count = CASE WHEN ? = 0 THEN 0 ELSE unread_count END,
             updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL",
    )
    .bind(unread_int)
    .bind(unread_int)
    .bind(updated_at)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    ensure_single_topic_change(changed.rows_affected(), topic_key)?;
    if !unread {
        clear_counted_unread_receipts(&mut tx, topic_key).await?;
    }
    HashAggregator::bubble_from_topic_for_key(&mut tx, topic_key).await?;
    let state = load_topic_unread_state(&mut tx, topic_key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(state)
}

async fn clear_counted_unread_receipts(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'message_unread_receipts'
        )",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        "UPDATE message_unread_receipts
         SET counted_unread = 0
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("清理话题未读收据计数失败：{e}"))
}

fn validate_topic_key(topic_key: &TopicKey) -> Result<(), String> {
    if topic_key.is_valid() {
        return Ok(());
    }
    Err(format!(
        "未读状态要求完整的话题身份：{}/{}/{}",
        topic_key.owner_type, topic_key.owner_id, topic_key.topic_id
    ))
}

fn validate_owner_key(topic_key: &TopicKey) -> Result<(), String> {
    if matches!(topic_key.owner_type.as_str(), "agent" | "group") && !topic_key.owner_id.is_empty()
    {
        return Ok(());
    }
    Err(format!(
        "所有者未读状态要求有效 owner 身份：{}/{}",
        topic_key.owner_type, topic_key.owner_id
    ))
}

fn ensure_single_topic_change(rows_affected: u64, topic_key: &TopicKey) -> Result<(), String> {
    if rows_affected == 1 {
        return Ok(());
    }
    Err(format!(
        "话题 {}/{}/{} 不存在、已删除或身份不唯一",
        topic_key.owner_type, topic_key.owner_id, topic_key.topic_id
    ))
}

async fn load_topic_unread_state(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
) -> Result<TopicUnreadState, String> {
    let row = sqlx::query(
        "SELECT unread, unread_count
         FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| format!("读取话题未读状态失败：{e}"))?;
    let owner_unread_count = load_owner_unread_count(tx, topic_key).await?;
    Ok(TopicUnreadState {
        owner_id: topic_key.owner_id.clone(),
        owner_type: topic_key.owner_type.clone(),
        topic_id: topic_key.topic_id.clone(),
        unread: row
            .try_get::<i32, _>("unread")
            .map_err(|e| format!("解码话题未读状态失败：{e}"))?
            != 0,
        unread_count: row
            .try_get("unread_count")
            .map_err(|e| format!("解码话题未读计数失败：{e}"))?,
        owner_unread_count,
    })
}

async fn load_owner_unread_count(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
) -> Result<i32, String> {
    let row = sqlx::query(
        "SELECT CAST(COALESCE(SUM(CASE WHEN unread = 1 THEN unread_count ELSE 0 END), 0) AS INTEGER) AS total_count,
                COALESCE(MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END), 0) AS has_unread
         FROM topics
         WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| format!("读取 owner 未读聚合失败：{e}"))?;
    let total_count: i64 = row
        .try_get("total_count")
        .map_err(|e| format!("解码 owner 未读计数失败：{e}"))?;
    let has_unread: i32 = row
        .try_get("has_unread")
        .map_err(|e| format!("解码 owner 未读标记失败：{e}"))?;
    if total_count > i64::from(i32::MAX) {
        return Err("owner 未读计数超出 i32 范围".to_string());
    }
    Ok(if total_count > 0 {
        total_count as i32
    } else if has_unread != 0 {
        -1
    } else {
        0
    })
}

/// 原子增加消息带来的未读数量，并在同一事务内刷新同步 hash。
#[tauri::command]
pub async fn increment_topic_unread_count(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    msg_id: String,
    mark_unread: Option<bool>,
) -> Result<TopicUnreadState, String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    record_topic_unread_for_message_in_pool(
        &db_state.pool,
        &topic_key,
        &msg_id,
        mark_unread.unwrap_or(true),
        crate::vcp_modules::infra::utils::now_millis(),
    )
    .await
}

/// Internal atomic API used by every production stream-event path.
pub(crate) async fn record_topic_unread_for_message_in_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_key: &TopicKey,
    msg_id: &str,
    mark_unread: bool,
    updated_at: i64,
) -> Result<TopicUnreadState, String> {
    validate_topic_key(topic_key)?;
    if msg_id.trim().is_empty() {
        return Err("消息未读记账要求非空 msg_id".to_string());
    }
    // Acquire SQLite's write lock before reading the topic or inserting the
    // receipt. A deferred transaction can establish a read snapshot first;
    // when several stream events then promote that snapshot to a writer,
    // SQLite may reject the promotion with SQLITE_BUSY_SNAPSHOT instead of
    // waiting for the other writer to commit. BEGIN IMMEDIATE serializes the
    // short receipt/count transaction while keeping both updates atomic.
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| e.to_string())?;
    let state =
        record_topic_unread_for_message_in_tx(&mut tx, topic_key, msg_id, mark_unread, updated_at)
            .await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(state)
}

/// Apply a message unread receipt inside a caller-owned write transaction.
///
/// Topic/message creation paths use this form so the message row, receipt,
/// unread count, and hash bubble commit or roll back together.
pub(crate) async fn record_topic_unread_for_message_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    msg_id: &str,
    mark_unread: bool,
    updated_at: i64,
) -> Result<TopicUnreadState, String> {
    validate_topic_key(topic_key)?;
    if msg_id.trim().is_empty() {
        return Err("消息未读记账要求非空 msg_id".to_string());
    }
    ensure_live_topic(tx, topic_key).await?;
    ensure_live_message(tx, topic_key, msg_id).await?;
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO message_unread_receipts
            (owner_type, owner_id, topic_id, msg_id, created_at, counted_unread)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .bind(msg_id)
    .bind(updated_at)
    .bind(i32::from(mark_unread))
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("记录消息未读收据失败：{e}"))?;

    if inserted.rows_affected() == 1 && mark_unread {
        let changed = sqlx::query(
            "UPDATE topics
             SET unread = 1, unread_count = unread_count + 1, updated_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL",
        )
        .bind(updated_at)
        .bind(&topic_key.owner_type)
        .bind(&topic_key.owner_id)
        .bind(&topic_key.topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| format!("增加话题未读计数失败：{e}"))?;
        ensure_single_topic_change(changed.rows_affected(), topic_key)?;
        HashAggregator::bubble_from_topic_for_key(tx, topic_key).await?;
    }
    let state = load_topic_unread_state(tx, topic_key).await?;
    Ok(state)
}

/// Compatibility name for internal tests/callers; production callers must
/// pass the complete message identity and explicit mark-unread intent.
pub(crate) async fn increment_topic_unread_count_in_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_key: &TopicKey,
    msg_id: &str,
    mark_unread: bool,
    updated_at: i64,
) -> Result<TopicUnreadState, String> {
    record_topic_unread_for_message_in_pool(pool, topic_key, msg_id, mark_unread, updated_at).await
}

async fn ensure_live_topic(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
) -> Result<(), String> {
    let exists: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| format!("读取话题未读状态失败：{e}"))?;
    if exists == Some(1) {
        Ok(())
    } else {
        Err(format!(
            "话题 {}/{}/{} 不存在、已删除或身份不唯一",
            topic_key.owner_type, topic_key.owner_id, topic_key.topic_id
        ))
    }
}

async fn ensure_live_message(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    msg_id: &str,
) -> Result<(), String> {
    let exists: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .bind(msg_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| format!("读取消息未读状态失败：{e}"))?;
    if exists == Some(1) {
        Ok(())
    } else {
        Err(format!(
            "消息 {}/{}/{}/{} 不存在、已删除或身份不唯一",
            topic_key.owner_type, topic_key.owner_id, topic_key.topic_id, msg_id
        ))
    }
}
