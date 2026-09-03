use super::{read_group_config, GroupManagerState};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn save_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group: GroupConfig,
) -> Result<bool, String> {
    if group.id.is_empty() {
        return Err("Group ID cannot be empty".to_string());
    }
    let group_id = group.id.clone();
    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;
    internal_write_group_config(&app_handle, &state, &group_id, &group, false, false).await
}

#[tauri::command]
pub async fn update_group_config(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
    updates: serde_json::Value,
) -> Result<GroupConfig, String> {
    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;
    let config = read_group_config(app_handle.clone(), state.clone(), group_id.clone()).await?;
    let mut value = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    if let (Some(target), Some(source)) = (value.as_object_mut(), updates.as_object()) {
        target.extend(
            source
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let config: GroupConfig = serde_json::from_value(value).map_err(|e| e.to_string())?;
    internal_write_group_config(&app_handle, &state, &group_id, &config, false, false).await?;
    Ok(config)
}

pub(crate) async fn internal_write_group_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
    config: &GroupConfig,
    skip_bubble: bool,
    _from_sync: bool,
) -> Result<bool, String> {
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let dto = GroupSyncDTO::from(config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    upsert_group_row(&mut tx, group_id, config, &config_hash, now).await?;
    replace_group_members(&mut tx, group_id, config, now).await?;
    upsert_group_topics(&mut tx, group_id, &config.topics, now).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    if !skip_bubble && !config.topics.is_empty() {
        let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
        HashAggregator::bubble_group_hash(&mut bubble_tx, group_id).await?;
        bubble_tx.commit().await.map_err(|e| e.to_string())?;
    }
    state.caches.insert(group_id.to_string(), config.clone());
    Ok(true)
}

async fn upsert_group_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    config_hash: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO groups (
            group_id, name, mode, group_prompt, invite_prompt, use_unified_model,
            unified_model, tag_match_mode, created_at, config_hash, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(group_id) DO UPDATE SET
            name = excluded.name, mode = excluded.mode,
            group_prompt = excluded.group_prompt, invite_prompt = excluded.invite_prompt,
            use_unified_model = excluded.use_unified_model, unified_model = excluded.unified_model,
            tag_match_mode = excluded.tag_match_mode, config_hash = excluded.config_hash,
            updated_at = excluded.updated_at",
    )
    .bind(group_id)
    .bind(&config.name)
    .bind(&config.mode)
    .bind(&config.group_prompt)
    .bind(&config.invite_prompt)
    .bind(i32::from(config.use_unified_model))
    .bind(&config.unified_model)
    .bind(&config.tag_match_mode)
    .bind(config.created_at)
    .bind(config_hash)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn replace_group_members(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    now: i64,
) -> Result<(), String> {
    sqlx::query("DELETE FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
    let member_tags = config.member_tags.as_ref().and_then(|v| v.as_object());
    for (index, agent_id) in config.members.iter().enumerate() {
        let tag = member_tags
            .and_then(|tags| tags.get(agent_id))
            .and_then(|value| value.as_str());
        sqlx::query(
            "INSERT INTO group_members
                (group_id, agent_id, member_tag, sort_order, updated_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(agent_id)
        .bind(tag)
        .bind(index as i32)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn upsert_group_topics(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    topics: &[crate::vcp_modules::topic_types::Topic],
    now: i64,
) -> Result<(), String> {
    for topic in topics {
        sqlx::query(
            "INSERT INTO topics (
                topic_id, owner_type, owner_id, title, created_at, updated_at, locked, unread
             ) VALUES (?, 'group', ?, ?, ?, ?, ?, ?)
             ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
                title = excluded.title, locked = excluded.locked, unread = excluded.unread,
                updated_at = excluded.updated_at",
        )
        .bind(&topic.id)
        .bind(group_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(now)
        .bind(topic.locked)
        .bind(topic.unread)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let key = TopicKey::new("group", group_id, topic.id.clone());
        HashAggregator::bubble_topic_hash_for_key(tx, &key).await?;
    }
    Ok(())
}
