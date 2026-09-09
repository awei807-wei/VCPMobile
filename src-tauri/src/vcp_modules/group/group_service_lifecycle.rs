use super::GroupManagerState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_service::{SyncCommand, SyncState};
use crate::vcp_modules::sync_types::{DeleteTarget, OwnerType};
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn create_group<R: Runtime>(
    app_handle: AppHandle<R>,
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
    let owner_lock = state.acquire_lock(&group_id).await;
    let _owner_guard = owner_lock.lock().await;
    let pool = &app_handle.state::<DbState>().pool;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let dto = GroupSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    insert_group_row(&mut tx, &group_id, &config, &config_hash, timestamp).await?;
    insert_group_topics(&mut tx, &group_id, &config.topics, timestamp).await?;
    HashAggregator::bubble_group_hash(&mut tx, &group_id).await?;
    let mut persisted_config = config;
    persisted_config.topics = load_group_topics(&mut tx, &group_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(persisted_config)
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
            (group_id, name, created_at, updated_at, mode, group_prompt, invite_prompt,
             use_unified_model, unified_model, tag_match_mode, config_hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(group_id)
    .bind(&config.name)
    .bind(config.created_at)
    .bind(timestamp)
    .bind(&config.mode)
    .bind(&config.group_prompt)
    .bind(&config.invite_prompt)
    .bind(i32::from(config.use_unified_model))
    .bind(&config.unified_model)
    .bind(&config.tag_match_mode)
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

async fn load_group_topics(
    tx: &mut Transaction<'_, Sqlite>,
    group_id: &str,
) -> Result<Vec<Topic>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC",
    )
    .bind(group_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let key = TopicKey::new("group", group_id, row.get::<String, _>("topic_id"));
            Topic {
                id: key.topic_id,
                name: row.get("title"),
                created_at: row.get("created_at"),
                locked: row.get::<i32, _>("locked") != 0,
                unread: row.get::<i32, _>("unread") != 0,
                unread_count: row.get("unread_count"),
                msg_count: row.get("msg_count"),
                owner_id: key.owner_id,
                owner_type: key.owner_type,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn delete_group<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<bool, String> {
    let owner_lock = state.acquire_lock(&group_id).await;
    let _owner_guard = owner_lock.lock().await;
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    delete_group_data(pool, &group_id, now).await?;
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

pub(crate) async fn delete_group_data(
    pool: &SqlitePool,
    group_id: &str,
    now: i64,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let result = sqlx::query(
        "UPDATE groups SET deleted_at = ?
         WHERE group_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    if result.rows_affected() != 1 {
        return Err(format!("群组 {group_id} 不存在或已删除"));
    }
    mark_group_topics_deleted(&mut tx, group_id, now).await?;
    mark_group_messages_deleted(&mut tx, group_id, now).await?;
    clear_group_unread_receipts(&mut tx, group_id).await?;
    clear_group_attachment_relations(&mut tx, group_id).await?;
    clear_group_generations(&mut tx, group_id).await?;
    clear_group_member_tags(&mut tx, group_id).await?;
    tx.commit().await.map_err(|e| e.to_string())
}

async fn clear_group_unread_receipts(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
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
         WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn mark_group_topics_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn mark_group_messages_deleted(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE messages SET deleted_at = ?
         WHERE owner_type = 'group' AND owner_id = ?
           AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_group_generations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_group_attachment_relations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_type = 'group' AND owner_id = ?",
    )
    .bind(group_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn clear_group_member_tags(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
) -> Result<(), String> {
    sqlx::query("DELETE FROM group_member_tags WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
