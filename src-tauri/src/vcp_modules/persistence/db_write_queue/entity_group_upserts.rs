use super::entity_upserts::{expected_config_matches, require_changed, require_non_empty};
use super::DbWriteQueue;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use crate::vcp_modules::sync_error::SyncErrorStage;
use crate::vcp_modules::sync_hash::HashAggregator;
use rusqlite::OptionalExtension;

impl DbWriteQueue {
    pub(super) fn rusqlite_upsert_group(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> rusqlite::Result<()> {
        Self::rusqlite_upsert_group_if_expected(tx, id, dto, None).map(|_| ())
    }

    pub(super) fn rusqlite_upsert_group_if_expected(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &GroupSyncDTO,
        expected_config_hash: Option<&str>,
    ) -> rusqlite::Result<()> {
        require_non_empty(id, "Group upsert requires a non-empty id")?;
        ensure_group_writeable(tx, id)?;
        if !expected_config_matches(tx, "groups", "group_id", id, expected_config_hash)? {
            log::warn!(
                "[DbWriteQueue] skipped stale Group pull for {id}: local config_hash changed"
            );
            return Err(DbWriteQueue::sync_snapshot_stale_error(
                format!("local Group {id} changed after the sync snapshot"),
                SyncErrorStage::OwnerMetadata,
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let member_tags = replace_group_member_tags(tx, id, dto, now)?;
        let mut effective_dto = dto.clone();
        effective_dto.member_tags = Some(member_tags);
        let config_hash = HashAggregator::compute_group_config_hash(&effective_dto);
        let changed = tx.execute(
            "INSERT INTO groups (
                group_id, name, mode, group_prompt, invite_prompt,
                use_unified_model, unified_model, tag_match_mode,
                created_at, config_hash, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(group_id) DO UPDATE SET
                name = excluded.name, mode = excluded.mode,
                group_prompt = excluded.group_prompt, invite_prompt = excluded.invite_prompt,
                use_unified_model = excluded.use_unified_model,
                unified_model = excluded.unified_model, tag_match_mode = excluded.tag_match_mode,
                created_at = excluded.created_at, config_hash = excluded.config_hash,
                updated_at = excluded.updated_at
             WHERE groups.deleted_at IS NULL",
            rusqlite::params![
                id,
                &dto.name,
                &dto.mode,
                &dto.group_prompt,
                &dto.invite_prompt,
                if dto.use_unified_model { 1 } else { 0 },
                &dto.unified_model,
                &dto.tag_match_mode,
                dto.created_at,
                &config_hash,
                now
            ],
        )?;
        require_changed(changed, format!("Group {id} is tombstoned"))?;
        replace_group_members(tx, id, dto, now)?;
        Ok(())
    }
}

fn replace_group_members(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    dto: &GroupSyncDTO,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM group_members WHERE group_id = ?", [id])?;
    for (sort_order, member) in dto.members.iter().enumerate() {
        tx.execute(
            "INSERT INTO group_members
             (group_id, agent_id, sort_order, updated_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![id, member, sort_order as i64, now],
        )?;
    }
    Ok(())
}

fn replace_group_member_tags(
    tx: &rusqlite::Transaction<'_>,
    group_id: &str,
    dto: &GroupSyncDTO,
    now: i64,
) -> rusqlite::Result<serde_json::Value> {
    let Some(value) = dto.member_tags.as_ref() else {
        let tags = load_group_member_tags(tx, group_id)?;
        return Ok(serde_json::Value::Object(tags));
    };
    let object = value
        .as_object()
        .ok_or_else(|| DbWriteQueue::sync_contract_error("memberTags 必须是对象"))?;
    let entries = validate_group_member_tags(object)?;
    tx.execute(
        "DELETE FROM group_member_tags WHERE group_id = ?",
        [group_id],
    )?;
    let mut tags = serde_json::Map::new();
    for (agent_id, tag) in entries {
        let Some(tag) = tag else { continue };
        tx.execute(
            "INSERT INTO group_member_tags (group_id, agent_id, member_tag, updated_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![group_id, agent_id, tag, now],
        )?;
        tags.insert(agent_id, serde_json::Value::String(tag));
    }
    Ok(serde_json::Value::Object(tags))
}

fn validate_group_member_tags(
    object: &serde_json::Map<String, serde_json::Value>,
) -> rusqlite::Result<Vec<(String, Option<String>)>> {
    let mut entries = Vec::with_capacity(object.len());
    for (agent_id, value) in object {
        if agent_id.trim().is_empty() {
            return Err(DbWriteQueue::sync_contract_error(
                "memberTags 的 agentId 不能为空",
            ));
        }
        let Some(tag) = value.as_str() else {
            if value.is_null() {
                entries.push((agent_id.clone(), None));
                continue;
            }
            return Err(DbWriteQueue::sync_contract_error(
                "memberTags 的值必须是字符串或 null",
            ));
        };
        if tag.trim().is_empty() {
            entries.push((agent_id.clone(), None));
            continue;
        }
        entries.push((agent_id.clone(), Some(tag.to_string())));
    }
    Ok(entries)
}

fn load_group_member_tags(
    tx: &rusqlite::Transaction<'_>,
    group_id: &str,
) -> rusqlite::Result<serde_json::Map<String, serde_json::Value>> {
    let rows = tx
        .prepare(
            "SELECT agent_id, member_tag FROM group_member_tags
             WHERE group_id = ? ORDER BY agent_id",
        )?
        .query_map([group_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(agent_id, tag)| (agent_id, serde_json::Value::String(tag)))
        .collect())
}

fn ensure_group_writeable(tx: &rusqlite::Transaction<'_>, group_id: &str) -> rusqlite::Result<()> {
    let deleted_at = tx
        .query_row(
            "SELECT deleted_at FROM groups WHERE group_id = ?",
            [group_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?;
    if deleted_at.flatten().is_some() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Group {group_id} is tombstoned"
        )));
    }
    Ok(())
}
