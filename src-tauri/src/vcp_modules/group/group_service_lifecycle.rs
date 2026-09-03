use super::GroupManagerState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType};
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn create_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    name: String,
) -> Result<GroupConfig, String> {
    let timestamp = crate::vcp_modules::infra::utils::now_millis();
    let base_id = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect::<String>();
    let group_id = format!("____{base_id}_{timestamp}");
    let config = default_group_config(&group_id, &name, timestamp);
    let pool = &app_handle.state::<DbState>().pool;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let dto = GroupSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    insert_group_row(&mut tx, &group_id, &config, &config_hash, timestamp).await?;
    insert_group_topics(&mut tx, &group_id, &config.topics, timestamp).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let mut bubble_tx = pool.begin().await.map_err(|e| e.to_string())?;
    HashAggregator::bubble_group_hash(&mut bubble_tx, &group_id).await?;
    bubble_tx.commit().await.map_err(|e| e.to_string())?;
    state.caches.insert(group_id, config.clone());
    Ok(config)
}

fn default_group_config(group_id: &str, name: &str, timestamp: i64) -> GroupConfig {
    GroupConfig {
        id: group_id.to_string(),
        name: name.to_string(),
        avatar_calculated_color: None,
        members: vec![],
        mode: "sequential".to_string(),
        member_tags: Some(serde_json::json!({})),
        group_prompt: Some(String::new()),
        invite_prompt: Some("现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。".to_string()),
        use_unified_model: false,
        unified_model: Some(String::new()),
        topics: vec![Topic {
            id: format!("group_topic_{timestamp}"),
            name: "主要群聊".to_string(),
            created_at: timestamp,
            locked: true,
            unread: false,
            unread_count: 0,
            msg_count: 0,
            owner_id: group_id.to_string(),
            owner_type: "group".to_string(),
        }],
        tag_match_mode: Some("strict".to_string()),
        created_at: timestamp,
    }
}

async fn insert_group_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    config_hash: &str,
    timestamp: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO groups
            (group_id, name, created_at, updated_at, mode, use_unified_model, config_hash)
         VALUES (?, ?, ?, ?, 'sequential', 0, ?)",
    )
    .bind(group_id)
    .bind(&config.name)
    .bind(timestamp)
    .bind(timestamp)
    .bind(config_hash)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn insert_group_topics(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    topics: &[Topic],
    timestamp: i64,
) -> Result<(), String> {
    for topic in topics {
        sqlx::query(
            "INSERT INTO topics (topic_id, owner_type, owner_id, title, created_at, updated_at)
             VALUES (?, 'group', ?, ?, ?, ?)",
        )
        .bind(&topic.id)
        .bind(group_id)
        .bind(&topic.name)
        .bind(topic.created_at)
        .bind(timestamp)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let key = TopicKey::new("group", group_id, topic.id.clone());
        HashAggregator::bubble_topic_hash_for_key(tx, &key).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_group(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<bool, String> {
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    sqlx::query("UPDATE groups SET deleted_at = ? WHERE group_id = ?")
        .bind(now)
        .bind(&group_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    mark_group_topics_deleted(pool, &group_id, now).await?;
    mark_group_messages_deleted(pool, &group_id, now).await?;
    clear_group_generations(pool, &group_id).await?;
    state.caches.remove(&group_id);
    state.locks.remove(&group_id);
    if let Some(sync_state) = app_handle.try_state::<SyncState>() {
        let _ = sync_state.ws_sender.send(SyncCommand::NotifyDelete {
            target: DeleteTarget::Owner {
                owner_type: OwnerType::Group,
                owner_id: group_id,
            },
            deleted_at: now,
        });
    }
    Ok(true)
}

async fn mark_group_topics_deleted(
    pool: &sqlx::SqlitePool,
    group_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn mark_group_messages_deleted(
    pool: &sqlx::SqlitePool,
    group_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ?
           AND topic_id IN (
               SELECT topic_id FROM topics
               WHERE owner_type = 'group' AND owner_id = ?
           )
           AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .bind(group_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_group_generations(pool: &sqlx::SqlitePool, group_id: &str) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = 'group' AND owner_id = ?
           AND topic_id IN (
               SELECT topic_id FROM topics
               WHERE owner_type = 'group' AND owner_id = ?
           )",
    )
    .bind(group_id)
    .bind(group_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}
