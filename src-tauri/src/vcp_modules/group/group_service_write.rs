use super::{read_group_config_locked, GroupManagerState};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::{Row, Sqlite, Transaction};
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn save_group_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, GroupManagerState>,
    group: GroupConfig,
) -> Result<bool, String> {
    if group.id.is_empty() {
        return Err("Group ID cannot be empty".to_string());
    }
    let group_id = group.id.clone();
    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;
    internal_write_group_config(&app_handle, &group_id, &group).await?;
    Ok(true)
}

#[tauri::command]
pub async fn update_group_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, GroupManagerState>,
    group_id: String,
    updates: serde_json::Value,
) -> Result<GroupConfig, String> {
    let mutex = state.acquire_lock(&group_id).await;
    let _lock = mutex.lock().await;
    let config = read_group_config_locked(&app_handle, &group_id).await?;
    let mut value = serde_json::to_value(&config).map_err(|e| e.to_string())?;
    if let (Some(target), Some(source)) = (value.as_object_mut(), updates.as_object()) {
        target.extend(
            source
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let config: GroupConfig = serde_json::from_value(value).map_err(|e| e.to_string())?;
    internal_write_group_config(&app_handle, &group_id, &config).await
}

pub(crate) async fn internal_write_group_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    group_id: &str,
    config: &GroupConfig,
) -> Result<GroupConfig, String> {
    let pool = &app_handle.state::<DbState>().pool;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let effective_config =
        write_group_config_in_transaction(&mut tx, group_id, config, now).await?;
    let dto = GroupSyncDTO::from(&effective_config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    upsert_group_row(&mut tx, group_id, &effective_config, &config_hash, now).await?;
    // Keep the owner hash in the same transaction as the group and its
    // persistent member tags. Dropping this transaction on a bubble failure
    // leaves both the old config and old hash intact.
    HashAggregator::bubble_group_hash(&mut tx, group_id).await?;
    let mut persisted_config = effective_config;
    persisted_config.topics = load_group_topics(&mut tx, group_id).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(persisted_config)
}

async fn write_group_config_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    now: i64,
) -> Result<GroupConfig, String> {
    ensure_group_writeable(tx, group_id).await?;
    replace_group_members(tx, group_id, config, now).await?;
    let member_tags = persist_group_member_tags(tx, group_id, config, now).await?;
    let mut effective_config = config.clone();
    effective_config.member_tags = Some(member_tags);
    Ok(effective_config)
}

async fn upsert_group_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    config_hash: &str,
    now: i64,
) -> Result<(), String> {
    let result = sqlx::query(
        "INSERT INTO groups (
            group_id, name, mode, group_prompt, invite_prompt, use_unified_model,
            unified_model, tag_match_mode, created_at, config_hash, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(group_id) DO UPDATE SET
            name = excluded.name, mode = excluded.mode,
            group_prompt = excluded.group_prompt, invite_prompt = excluded.invite_prompt,
            use_unified_model = excluded.use_unified_model, unified_model = excluded.unified_model,
            tag_match_mode = excluded.tag_match_mode, config_hash = excluded.config_hash,
            updated_at = excluded.updated_at
         WHERE groups.deleted_at IS NULL",
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
    .map_err(|e| e.to_string())?;
    if result.rows_affected() != 1 {
        return Err(format!("群组 {group_id} 已删除或写入影响行数异常"));
    }
    Ok(())
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
    for (index, agent_id) in config.members.iter().enumerate() {
        sqlx::query(
            "INSERT INTO group_members
                (group_id, agent_id, sort_order, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(agent_id)
        .bind(index as i32)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn persist_group_member_tags(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
    config: &GroupConfig,
    now: i64,
) -> Result<serde_json::Value, String> {
    let Some(value) = config.member_tags.as_ref() else {
        let rows = sqlx::query(
            "SELECT agent_id, member_tag FROM group_member_tags
                 WHERE group_id = ? ORDER BY agent_id",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let mut tags = serde_json::Map::new();
        for row in rows {
            let agent_id: String = row.get("agent_id");
            let tag: String = row.get("member_tag");
            tags.insert(agent_id, serde_json::Value::String(tag));
        }
        return Ok(serde_json::Value::Object(tags));
    };
    let object = value
        .as_object()
        .ok_or_else(|| "memberTags 必须是对象".to_string())?;
    let entries = validate_group_member_tags(object)?;
    sqlx::query("DELETE FROM group_member_tags WHERE group_id = ?")
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
    let mut tags = serde_json::Map::new();
    for (agent_id, tag) in entries {
        let Some(tag) = tag else { continue };
        sqlx::query(
            "INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(&agent_id)
        .bind(&tag)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        tags.insert(agent_id, serde_json::Value::String(tag));
    }
    Ok(serde_json::Value::Object(tags))
}

fn validate_group_member_tags(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut entries = Vec::with_capacity(object.len());
    for (agent_id, value) in object {
        if agent_id.trim().is_empty() {
            return Err("memberTags 的 agentId 不能为空".to_string());
        }
        let Some(tag) = value.as_str() else {
            if value.is_null() {
                entries.push((agent_id.clone(), None));
                continue;
            }
            return Err("memberTags 的值必须是字符串或 null".to_string());
        };
        if tag.trim().is_empty() {
            entries.push((agent_id.clone(), None));
            continue;
        }
        entries.push((agent_id.clone(), Some(tag.to_string())));
    }
    Ok(entries)
}

async fn ensure_group_writeable(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_id: &str,
) -> Result<(), String> {
    let deleted_at: Option<Option<i64>> =
        sqlx::query_scalar("SELECT deleted_at FROM groups WHERE group_id = ?")
            .bind(group_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
    if deleted_at.flatten().is_some() {
        return Err(format!("群组 {group_id} 已删除"));
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

#[cfg(test)]
#[path = "group_service_write_tests.rs"]
mod tests;
