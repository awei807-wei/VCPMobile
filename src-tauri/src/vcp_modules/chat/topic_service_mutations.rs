use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::DeleteTarget;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn create_topic(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    name: String,
) -> Result<Topic, String> {
    let now = crate::vcp_modules::infra::utils::now_millis();
    let id = if owner_type == "group" {
        format!("group_topic_{now}")
    } else {
        format!("topic_{now}")
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
    sqlx::query(
        "INSERT INTO topics (topic_id, owner_id, owner_type, title, created_at, updated_at, msg_count, locked, unread, unread_count)
         VALUES (?, ?, ?, ?, ?, ?, 0, 1, 0, 0)",
    )
    .bind(&id)
    .bind(&owner_id)
    .bind(&owner_type)
    .bind(&name)
    .bind(now)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|e| format!("[CreateTopic] DB initialization failed: {e}"))?;

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    if let Err(e) = HashAggregator::bubble_from_topic_for_key(&mut tx, &topic_key).await {
        log::error!("[CreateTopic] Failed to bubble hash for topic {id}: {e}");
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(topic)
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
    delete_topic_rows(&db_state.pool, &topic_key, now).await?;

    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Topic(topic_key.clone()),
            deleted_at: now,
        });
    }
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    match topic_key.owner_type.as_str() {
        "agent" => {
            let _ = HashAggregator::bubble_agent_hash(&mut tx, &topic_key.owner_id).await;
        }
        "group" => {
            let _ = HashAggregator::bubble_group_hash(&mut tx, &topic_key.owner_id).await;
        }
        _ => {}
    }
    let _ = tx.commit().await;
    Ok(())
}

async fn delete_topic_rows(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
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
    sqlx::query(
        "UPDATE topics SET title = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&title)
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, &topic_key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
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
    sqlx::query(
        "UPDATE topics SET locked = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(locked)
    .bind(now)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_from_topic_for_key(&mut tx, &topic_key).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn set_topic_unread(
    _app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    unread: bool,
) -> Result<(), String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    set_topic_unread_in_pool(
        &db_state.pool,
        &topic_key,
        unread,
        crate::vcp_modules::infra::utils::now_millis(),
    )
    .await
}

pub(crate) async fn set_topic_unread_in_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    topic_key: &TopicKey,
    unread: bool,
    updated_at: i64,
) -> Result<(), String> {
    let unread_int = i32::from(unread);
    sqlx::query(
        "UPDATE topics
         SET unread = ?,
             unread_count = CASE WHEN ? = 0 THEN 0 ELSE unread_count END,
             updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(unread_int)
    .bind(unread_int)
    .bind(updated_at)
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
