use super::AgentConfigState;
use crate::vcp_modules::agent_types::{AgentConfig, AgentListItem};
use crate::vcp_modules::chat::topic_service::unread_owner_key;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group::group_service::GroupManagerState;
use crate::vcp_modules::group::group_types::{GroupConfig, GroupListItem};
use sqlx::{Row, Sqlite, Transaction};
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
    _agent_state: State<'_, AgentConfigState>,
    _group_state: State<'_, GroupManagerState>,
    db_state: State<'_, DbState>,
) -> Result<AssistantsSnapshot, String> {
    let mut tx = db_state.pool.begin().await.map_err(|e| e.to_string())?;
    let agents = load_agent_list(&mut tx).await?;
    let groups = load_group_list(&mut tx).await?;
    let unread_counts = load_unread_counts(&mut tx).await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(AssistantsSnapshot {
        agents,
        groups,
        unread_counts,
    })
}

async fn load_agent_list(tx: &mut Transaction<'_, Sqlite>) -> Result<Vec<AgentListItem>, String> {
    let rows = sqlx::query(
        "SELECT a.agent_id, a.name, a.system_prompt, a.mobile_system_prompt, a.model,
                a.temperature, a.context_token_limit, a.max_output_tokens, a.stream_output,
                a.use_temperature, av.dominant_color
         FROM agents a
         LEFT JOIN avatars av ON av.owner_id = a.agent_id
            AND av.owner_type = 'agent' AND av.deleted_at IS NULL
         WHERE a.deleted_at IS NULL",
    )
    .fetch_all(&mut **tx)
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
        result.push(AgentListItem {
            id: agent_id,
            name,
            model,
            avatar_calculated_color,
        });
    }
    Ok(result)
}

async fn load_group_list(tx: &mut Transaction<'_, Sqlite>) -> Result<Vec<GroupListItem>, String> {
    let group_rows = sqlx::query(
        "SELECT g.group_id, g.name, g.mode, g.group_prompt, g.invite_prompt,
                g.use_unified_model, g.unified_model, g.tag_match_mode, g.created_at,
                av.dominant_color
         FROM groups g
         LEFT JOIN avatars av ON av.owner_id = g.group_id
            AND av.owner_type = 'group' AND av.deleted_at IS NULL
         WHERE g.deleted_at IS NULL",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    let member_rows = sqlx::query(
        "SELECT group_id, agent_id
         FROM group_members ORDER BY group_id, sort_order ASC",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    let tag_rows = sqlx::query(
        "SELECT group_id, agent_id, member_tag
         FROM group_member_tags ORDER BY group_id, agent_id ASC",
    )
    .fetch_all(&mut **tx)
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
    }
    for row in tag_rows {
        let group_id: String = row.get("group_id");
        let agent_id: String = row.get("agent_id");
        let tag: String = row.get("member_tag");
        tags_by_group
            .entry(group_id)
            .or_default()
            .insert(agent_id, serde_json::Value::String(tag));
    }
    Ok(build_group_list(
        group_rows,
        &mut members_by_group,
        &mut tags_by_group,
    ))
}

fn build_group_list(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    members_by_group: &mut HashMap<String, Vec<String>>,
    tags_by_group: &mut HashMap<String, serde_json::Map<String, serde_json::Value>>,
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
        result.push(GroupListItem {
            id: group_id,
            name,
            avatar_calculated_color,
            members,
        });
    }
    result
}

async fn load_unread_counts(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<HashMap<String, i32>, String> {
    let rows = sqlx::query(
        "SELECT owner_type, owner_id,
                CAST(COALESCE(SUM(CASE WHEN unread = 1 THEN unread_count ELSE 0 END), 0) AS INTEGER) AS total_count,
                MAX(CASE WHEN unread = 1 THEN 1 ELSE 0 END) AS has_unread
         FROM topics WHERE deleted_at IS NULL
         GROUP BY owner_type, owner_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    let mut result = HashMap::new();
    for row in rows {
        let owner_type: String = row.get("owner_type");
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
            result.insert(unread_owner_key(&owner_type, &owner_id), value);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::load_unread_counts;
    use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

    #[tokio::test]
    async fn unread_counts_are_scoped_by_owner_type_for_shared_owner_ids() {
        let pool: SqlitePool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                unread INTEGER NOT NULL DEFAULT 0,
                unread_count INTEGER NOT NULL DEFAULT 0,
                deleted_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO topics (owner_type, owner_id, unread, unread_count)
             VALUES
                ('agent', 'shared-owner', 1, 3),
                ('group', 'shared-owner', 1, 99),
                ('group', 'group-only', 1, 0),
                ('group', 'x/y', 1, 7),
                ('agent', 'group:x%2Fy', 1, 11)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let counts = load_unread_counts(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(counts.get("agent:shared-owner"), Some(&3));
        assert!(!counts.contains_key("shared-owner"));
        assert_eq!(counts.get("group:shared-owner"), Some(&99));
        assert_eq!(counts.get("group:group-only"), Some(&-1));
        assert_eq!(counts.get("group:x%2Fy"), Some(&7));
        assert_eq!(counts.get("agent:group%3Ax%252Fy"), Some(&11));
    }
}
