use super::AgentConfigState;
use crate::vcp_modules::agent_types::{AgentConfig, AgentListItem};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_service::GroupManagerState;
use crate::vcp_modules::group::group_types::{GroupConfig, GroupListItem};
use sqlx::Row;
use std::collections::HashMap;
use tauri::State;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssistantsSnapshot {
    pub agents: Vec<AgentListItem>,
    pub groups: Vec<GroupListItem>,
    pub unread_counts: HashMap<String, i32>,
}

#[tauri::command]
pub async fn get_assistants_snapshot(
    agent_state: State<'_, AgentConfigState>,
    group_state: State<'_, GroupManagerState>,
    db_state: State<'_, DbState>,
) -> Result<AssistantsSnapshot, String> {
    let agents = load_agent_list(&db_state.pool, &agent_state).await?;
    let groups = load_group_list(&db_state.pool, &group_state).await?;
    let unread_counts = load_unread_counts(&db_state.pool).await?;
    Ok(AssistantsSnapshot {
        agents,
        groups,
        unread_counts,
    })
}

async fn load_agent_list(
    pool: &sqlx::SqlitePool,
    state: &AgentConfigState,
) -> Result<Vec<AgentListItem>, String> {
    let rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.system_prompt, a.mobile_system_prompt, a.model,
                a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output,
                a.use_temperature, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id AND av.owner_type = 'agent'
         WHERE a.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let agent_id: String = row.get("agent_id");
        let name: String = row.get("name");
        let model: String = row.get("model");
        let config = AgentConfig {
            id: agent_id.clone(),
            name: name.clone(),
            system_prompt: row.get("system_prompt"),
            mobile_system_prompt: row.get("mobile_system_prompt"),
            model: model.clone(),
            temperature: row.get("temperature"),
            context_token_limit: row.get("context_token_limit"),
            max_output_tokens: row.get("max_output_tokens"),
            stream_output: row.get::<i32, _>("stream_output") != 0,
            use_temperature: row.get::<i32, _>("use_temperature") != 0,
            avatar_calculated_color: row.get("dominant_color"),
            topics: vec![],
        };
        let avatar_calculated_color = config.avatar_calculated_color.clone();
        state.caches.insert(agent_id.clone(), config);
        result.push(AgentListItem {
            id: agent_id,
            name,
            model,
            avatar_calculated_color,
        });
    }
    Ok(result)
}

async fn load_group_list(
    pool: &sqlx::SqlitePool,
    state: &GroupManagerState,
) -> Result<Vec<GroupListItem>, String> {
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, g.mode, g.group_prompt, g.invite_prompt,
                g.use_unified_model, g.unified_model, g.tag_match_mode, g.created_at,
                av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id AND av.owner_type = 'group'
         WHERE g.deleted_at IS NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let member_rows = sqlx::query(
        "SELECT group_id, agent_id, member_tag
         FROM group_members ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut members_by_group: HashMap<String, Vec<String>> = HashMap::new();
    let mut tags_by_group: HashMap<String, serde_json::Map<String, serde_json::Value>> =
        HashMap::new();
    for row in member_rows {
        let group_id: String = row.get("group_id");
        let agent_id: String = row.get("agent_id");
        members_by_group
            .entry(group_id.clone())
            .or_default()
            .push(agent_id.clone());
        if let Some(tag) = row.get::<Option<String>, _>("member_tag") {
            tags_by_group
                .entry(group_id)
                .or_default()
                .insert(agent_id, serde_json::Value::String(tag));
        }
    }
    Ok(build_group_list(
        group_rows,
        &mut members_by_group,
        &mut tags_by_group,
        state,
    ))
}

fn build_group_list(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    members_by_group: &mut HashMap<String, Vec<String>>,
    tags_by_group: &mut HashMap<String, serde_json::Map<String, serde_json::Value>>,
    state: &GroupManagerState,
) -> Vec<GroupListItem> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let group_id: String = row.get("group_id");
        let members = members_by_group.remove(&group_id).unwrap_or_default();
        let member_tags = tags_by_group.remove(&group_id).unwrap_or_default();
        let config = GroupConfig {
            id: group_id.clone(),
            name: row.get("name"),
            avatar_calculated_color: row.get("dominant_color"),
            members: members.clone(),
            mode: row.get("mode"),
            member_tags: Some(serde_json::Value::Object(member_tags)),
            group_prompt: row.get("group_prompt"),
            invite_prompt: row.get("invite_prompt"),
            use_unified_model: row.get::<i32, _>("use_unified_model") != 0,
            unified_model: row.get("unified_model"),
            topics: vec![],
            tag_match_mode: row.get("tag_match_mode"),
            created_at: row.get("created_at"),
        };
        let avatar_calculated_color = config.avatar_calculated_color.clone();
        let name = config.name.clone();
        state.caches.insert(group_id.clone(), config);
        result.push(GroupListItem {
            id: group_id,
            name,
            avatar_calculated_color,
            members,
        });
    }
    result
}

async fn load_unread_counts(pool: &sqlx::SqlitePool) -> Result<HashMap<String, i32>, String> {
    let rows = sqlx::query(
        "SELECT owner_type, owner_id,
                CAST(COALESCE(SUM(unread_count), 0) AS INTEGER) AS total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) AS has_unread
         FROM topics WHERE deleted_at IS NULL
         GROUP BY owner_type, owner_id",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    for row in rows {
        let owner_id: String = row.get("owner_id");
        let total_count: i64 = row.get("total_count");
        let has_unread: i32 = row.get("has_unread");
        let value = if total_count > 0 {
            total_count as i32
        } else if has_unread != 0 {
            -1
        } else {
            0
        };
        if value != 0 {
            result.insert(owner_id, value);
        }
    }
    Ok(result)
}
