use super::GroupManagerState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_types::GroupConfig;
use sqlx::Row;
use std::collections::HashMap;
use tauri::{AppHandle, Manager, State};

type GroupMemberIndex = HashMap<String, Vec<String>>;
type GroupTagIndex = HashMap<String, serde_json::Map<String, serde_json::Value>>;

#[tauri::command]
pub async fn get_groups(
    app_handle: AppHandle,
    state: State<'_, GroupManagerState>,
) -> Result<Vec<GroupConfig>, String> {
    let start_total = std::time::Instant::now();
    let pool = &app_handle.state::<DbState>().pool;
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
    if group_rows.is_empty() {
        log::info!(
            "[Profile] get_groups total: {}ms (empty)",
            start_total.elapsed().as_millis()
        );
        return Ok(Vec::new());
    }
    let member_rows = sqlx::query(
        "SELECT group_id, agent_id, member_tag
         FROM group_members ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    let (mut members_by_group, mut tags_by_group) = index_members(member_rows);
    let groups = build_group_configs(
        group_rows,
        &mut members_by_group,
        &mut tags_by_group,
        &state,
    );
    log::info!(
        "[Profile] get_groups finished. Total: {}ms | Groups: {}",
        start_total.elapsed().as_millis(),
        groups.len()
    );
    Ok(groups)
}

fn build_group_configs(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    members_by_group: &mut GroupMemberIndex,
    tags_by_group: &mut GroupTagIndex,
    state: &GroupManagerState,
) -> Vec<GroupConfig> {
    let mut groups = Vec::with_capacity(rows.len());
    for row in rows {
        let group_id: String = row.get("group_id");
        let members = members_by_group.remove(&group_id).unwrap_or_default();
        let member_tags = tags_by_group.remove(&group_id).unwrap_or_default();
        let config = GroupConfig {
            id: group_id.clone(),
            name: row.get("name"),
            avatar_calculated_color: row.get("dominant_color"),
            members,
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
        state.caches.insert(group_id, config.clone());
        groups.push(config);
    }
    groups
}

fn index_members(rows: Vec<sqlx::sqlite::SqliteRow>) -> (GroupMemberIndex, GroupTagIndex) {
    let mut members_by_group: GroupMemberIndex = HashMap::new();
    let mut tags_by_group: GroupTagIndex = HashMap::new();
    for row in rows {
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
    (members_by_group, tags_by_group)
}
