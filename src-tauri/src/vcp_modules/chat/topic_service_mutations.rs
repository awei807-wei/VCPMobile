use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::DeleteTarget;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::{Sqlite, Transaction};
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Copy)]
pub(crate) enum CommitMode {
    Real,
    #[cfg(test)]
    Fail,
}

#[tauri::command]
pub async fn create_topic(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    name: String,
) -> Result<Topic, String> {
    let now = crate::vcp_modules::infra::utils::now_millis();
    create_topic_in_pool(&db_state.pool, owner_id, owner_type, name, now).await
}

pub(crate) async fn create_topic_in_pool(
    pool: &sqlx::SqlitePool,
    owner_id: String,
    owner_type: String,
    name: String,
    now: i64,
) -> Result<Topic, String> {
    let id = if owner_type == "group" {
        format!("group_topic_{now}_{}", uuid::Uuid::new_v4().simple())
    } else {
        format!("topic_{now}_{}", uuid::Uuid::new_v4().simple())
    };
    let topic = Topic {
        id: id.clone(),
        name: name.clone(),
        created_at: now,
        locked: true,
        unread: false,
        unread_count: 0,
        msg_count: 0,
        owner_id: owner_id.clone(),
        owner_type: owner_type.clone(),
    };
    let topic_key = TopicKey::new(owner_type.clone(), owner_id.clone(), id.clone());
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    insert_topic_row(&mut tx, &topic_key, &name, now).await?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, &topic_key).await?;
    commit_transaction(tx, CommitMode::Real).await?;
    Ok(topic)
}

async fn insert_topic_row(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    name: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 1, 0, 0)",
    )
    .bind(&topic_key.topic_id)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.owner_type)
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|e| format!("[CreateTopic] DB initialization failed: {e}"))
    .and_then(|result| {
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(format!(
                "[CreateTopic] topic {}/{} insertion affected {} rows",
                topic_key.owner_id,
                topic_key.topic_id,
                result.rows_affected()
            ))
        }
    })
}

async fn commit_transaction(tx: Transaction<'_, Sqlite>, mode: CommitMode) -> Result<(), String> {
    #[cfg(test)]
    let mut tx = tx;
    #[cfg(not(test))]
    let _ = mode;
    #[cfg(test)]
    if matches!(mode, CommitMode::Fail) {
        sqlx::query(
            "INSERT INTO topic_commit_failure_child(id, parent_id)
             VALUES (1, 999999)",
        )
        .execute(&mut *tx)
        .await
        .map_err(|error| format!("测试提交失败注入失败：{error}"))?;
    }
    tx.commit().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_topic(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    let notify_key = topic_key.clone();
    let notify_app = app_handle.clone();
    delete_topic_with_notify(
        &db_state.pool,
        &topic_key,
        now,
        CommitMode::Real,
        move || {
            notify_topic_deleted(&notify_app, &notify_key, now);
        },
    )
    .await
}

pub(crate) async fn delete_topic_with_notify<F>(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    now: i64,
    commit_mode: CommitMode,
    on_commit: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    delete_topic_rows(&mut tx, topic_key, now).await?;
    bubble_owner_hash(&mut tx, topic_key).await?;
    commit_transaction(tx, commit_mode).await?;
    on_commit();
    Ok(())
}

fn notify_topic_deleted(app_handle: &AppHandle, topic_key: &TopicKey, deleted_at: i64) {
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        if let Err(error) = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Topic(topic_key.clone()),
            deleted_at,
        }) {
            log::warn!(
                "[DeleteTopic] notification failed after commit: owner_type={}, owner_id={}, topic_id={}, deleted_at={}, error={error}",
                topic_key.owner_type,
                topic_key.owner_id,
                topic_key.topic_id,
                deleted_at,
            );
        }
    }
}

async fn delete_topic_rows(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
    now: i64,
) -> Result<(), String> {
    let changed = sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    ensure_single_topic_change(changed.rows_affected(), topic_key)?;
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    clear_topic_unread_receipts(tx, topic_key).await?;
    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

async fn clear_topic_unread_receipts(
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
    .map_err(|error| error.to_string())?;
    if !exists {
        return Ok(());
    }
    sqlx::query(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn bubble_owner_hash(
    tx: &mut Transaction<'_, Sqlite>,
    topic_key: &TopicKey,
) -> Result<(), String> {
    match topic_key.owner_type.as_str() {
        "agent" => HashAggregator::bubble_agent_hash(tx, &topic_key.owner_id).await,
        "group" => HashAggregator::bubble_group_hash(tx, &topic_key.owner_id).await,
        other => Err(format!(
            "Topic {} has unsupported owner type {other}",
            topic_key.topic_id
        )),
    }
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

#[tauri::command]
pub async fn update_topic_title(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    title: String,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    update_topic_title_in_pool(&db_state.pool, &topic_key, &title, now).await
}

pub(crate) async fn update_topic_title_in_pool(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    title: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let changed = sqlx::query(
        "UPDATE topics SET title = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(title)
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    ensure_single_topic_change(changed.rows_affected(), topic_key)?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, topic_key).await?;
    commit_transaction(tx, CommitMode::Real).await?;
    Ok(())
}

#[tauri::command]
pub async fn summarize_topic(
    app_handle: AppHandle,
    settings_state: State<'_, SettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    agent_name: String,
) -> Result<String, String> {
    crate::vcp_modules::topic_summary_service::summarize_topic(
        app_handle,
        settings_state,
        owner_id,
        owner_type,
        topic_id,
        agent_name,
    )
    .await
}

#[tauri::command]
pub async fn toggle_topic_lock(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    locked: bool,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    toggle_topic_lock_in_pool(&db_state.pool, &topic_key, locked, now).await
}

pub(crate) async fn toggle_topic_lock_in_pool(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    locked: bool,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let changed = sqlx::query(
        "UPDATE topics SET locked = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(locked)
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    ensure_single_topic_change(changed.rows_affected(), topic_key)?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, topic_key).await?;
    commit_transaction(tx, CommitMode::Real).await?;
    Ok(())
}
