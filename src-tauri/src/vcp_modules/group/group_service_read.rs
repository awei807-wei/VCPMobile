use super::GroupManagerState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::topic_types::{Topic, TopicKey};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime, State};

#[tauri::command]
pub async fn read_group_config<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, GroupManagerState>,
    group_id: String,
) -> Result<GroupConfig, String> {
    read_group_config_internal(&app_handle, &state, &group_id).await
}

pub async fn read_group_config_internal<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &GroupManagerState,
    group_id: &str,
) -> Result<GroupConfig, String> {
    if let Some(cached) = state.caches.get(group_id) {
        return Ok(cached.value().clone());
    }
    let pool = &app_handle.state::<DbState>().pool;
    let row = sqlx::query(
        "SELECT g.name, g.mode, g.group_prompt, g.invite_prompt, g.use_unified_model,
                g.unified_model, g.tag_match_mode, g.created_at, av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group'
         WHERE g.group_id = ? AND g.deleted_at IS NULL",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let Some(row) = row else {
        return Err(format!("Group {group_id} not found"));
    };

    let (members, member_tags) = load_group_members(pool, group_id).await?;
    let topics = load_group_topics(pool, group_id).await?;
    let config = GroupConfig {
        id: group_id.to_string(),
        name: row.get("name"),
        avatar_calculated_color: row.get("dominant_color"),
        members,
        mode: row.get("mode"),
        member_tags: Some(serde_json::Value::Object(member_tags)),
        group_prompt: row.get("group_prompt"),
        invite_prompt: row.get("invite_prompt"),
        use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
        unified_model: row.get("unified_model"),
        topics,
        tag_match_mode: row.get("tag_match_mode"),
        created_at: row.get("created_at"),
    };
    state.caches.insert(group_id.to_string(), config.clone());
    Ok(config)
}

async fn load_group_members(
    pool: &sqlx::SqlitePool,
    group_id: &str,
) -> Result<(Vec<String>, serde_json::Map<String, serde_json::Value>), String> {
    let rows = sqlx::query(
        "SELECT agent_id, member_tag FROM group_members
         WHERE group_id = ? ORDER BY sort_order ASC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(read_members(rows))
}

async fn load_group_topics(pool: &sqlx::SqlitePool, group_id: &str) -> Result<Vec<Topic>, String> {
    let rows = sqlx::query(
        "SELECT topic_id, title, created_at, locked, unread, unread_count, msg_count
         FROM topics
         WHERE owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL
         ORDER BY updated_at DESC",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| topic_from_row(&row, group_id))
        .collect())
}

fn read_members(
    rows: Vec<sqlx::sqlite::SqliteRow>,
) -> (Vec<String>, serde_json::Map<String, serde_json::Value>) {
    let mut members = Vec::with_capacity(rows.len());
    let mut tags = serde_json::Map::new();
    for row in rows {
        let agent_id: String = row.get("agent_id");
        members.push(agent_id.clone());
        if let Some(tag) = row.get::<Option<String>, _>("member_tag") {
            tags.insert(agent_id, serde_json::Value::String(tag));
        }
    }
    (members, tags)
}

fn topic_from_row(row: &sqlx::sqlite::SqliteRow, group_id: &str) -> Topic {
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
}
