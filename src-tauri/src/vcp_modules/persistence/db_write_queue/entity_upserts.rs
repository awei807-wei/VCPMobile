use super::DbWriteQueue;

use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_hash::HashAggregator;
use rusqlite::OptionalExtension;

impl DbWriteQueue {
    pub(super) fn rusqlite_upsert_agent(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> rusqlite::Result<()> {
        require_non_empty(id, "Agent upsert requires a non-empty id")?;
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_agent_config_hash(dto);
        let changed = tx.execute(
            "INSERT INTO agents (
                agent_id, name, system_prompt, model, temperature,
                context_token_limit, max_output_tokens,
                stream_output, config_hash, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(agent_id) DO UPDATE SET
                name = excluded.name, system_prompt = excluded.system_prompt,
                model = excluded.model, temperature = excluded.temperature,
                context_token_limit = excluded.context_token_limit,
                max_output_tokens = excluded.max_output_tokens,
                stream_output = excluded.stream_output,
                config_hash = excluded.config_hash, updated_at = excluded.updated_at
             WHERE agents.deleted_at IS NULL",
            rusqlite::params![
                id,
                &dto.name,
                &dto.system_prompt,
                &dto.model,
                dto.temperature,
                dto.context_token_limit,
                dto.max_output_tokens,
                if dto.stream_output { 1 } else { 0 },
                &config_hash,
                now
            ],
        )?;
        require_changed(changed, format!("Agent {id} is tombstoned"))
    }

    pub(super) fn rusqlite_upsert_group(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &GroupSyncDTO,
    ) -> rusqlite::Result<()> {
        require_non_empty(id, "Group upsert requires a non-empty id")?;
        let now = chrono::Utc::now().timestamp_millis();
        let config_hash = HashAggregator::compute_group_config_hash(dto);
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
        replace_group_members(tx, id, dto, now)
    }

    pub(super) fn rusqlite_upsert_avatar(
        tx: &rusqlite::Transaction<'_>,
        owner_type: &str,
        owner_id: &str,
        bytes: &[u8],
    ) -> rusqlite::Result<()> {
        if !is_valid_avatar_owner(owner_type, owner_id) {
            return Err(DbWriteQueue::sync_contract_error(
                "Avatar requires a non-empty owner id and a supported owner type",
            ));
        }
        if !live_parent(tx, owner_type, owner_id)? {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Avatar owner {owner_type}/{owner_id} is missing or deleted"
            )));
        }
        let hash = HashAggregator::compute_avatar_hash(bytes);
        let now = chrono::Utc::now().timestamp_millis();
        let changed = tx.execute(
            "INSERT INTO avatars (
                owner_type, owner_id, avatar_hash, mime_type, image_data,
                dominant_color, updated_at
            ) VALUES (?, ?, ?, 'image/png', ?, ?, ?)
             ON CONFLICT(owner_type, owner_id) DO UPDATE SET
                avatar_hash = excluded.avatar_hash, image_data = excluded.image_data,
                dominant_color = excluded.dominant_color, updated_at = excluded.updated_at
             WHERE avatars.deleted_at IS NULL",
            rusqlite::params![
                owner_type,
                owner_id,
                &hash,
                bytes,
                Option::<String>::None,
                now
            ],
        )?;
        require_changed(
            changed,
            format!("Avatar {owner_type}/{owner_id} is tombstoned"),
        )
    }

    pub(super) fn rusqlite_upsert_agent_topic(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
        dto: &AgentTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        validate_topic_input(topic_id, &dto.id, &dto.owner_id, "Agent topic")?;
        validate_live_owner(tx, "agents", &dto.owner_id, "Agent topic", topic_id)?;
        validate_existing_topic(tx, topic_id, &dto.owner_id, "agent", "Agent topic")?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = tx.execute(
            "INSERT INTO topics (
                topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at
            )
            SELECT ?, ?, ?, 'agent', ?, ?, ?, ?
            WHERE EXISTS (SELECT 1 FROM agents WHERE agent_id = ? AND deleted_at IS NULL)
            ON CONFLICT(topic_id) DO UPDATE SET
                title = excluded.title, locked = excluded.locked,
                unread = excluded.unread, updated_at = excluded.updated_at",
            rusqlite::params![
                topic_id,
                &dto.name,
                &dto.owner_id,
                dto.created_at,
                if dto.locked { 1 } else { 0 },
                if dto.unread { 1 } else { 0 },
                now,
                &dto.owner_id
            ],
        )?;
        require_changed(
            changed,
            format!("Agent topic {topic_id} upsert affected {changed} rows"),
        )
    }

    pub(super) fn rusqlite_upsert_group_topic(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
        dto: &GroupTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        validate_topic_input(topic_id, &dto.id, &dto.owner_id, "Group topic")?;
        validate_live_owner(tx, "groups", &dto.owner_id, "Group topic", topic_id)?;
        validate_existing_topic(tx, topic_id, &dto.owner_id, "group", "Group topic")?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = tx.execute(
            "INSERT INTO topics (
                topic_id, title, owner_id, owner_type, created_at, locked, unread, updated_at
            )
            SELECT ?, ?, ?, 'group', ?, 1, 0, ?
            WHERE EXISTS (SELECT 1 FROM groups WHERE group_id = ? AND deleted_at IS NULL)
            ON CONFLICT(topic_id) DO UPDATE SET
                title = excluded.title, updated_at = excluded.updated_at",
            rusqlite::params![
                topic_id,
                &dto.name,
                &dto.owner_id,
                dto.created_at,
                now,
                &dto.owner_id
            ],
        )?;
        require_changed(
            changed,
            format!("Group topic {topic_id} upsert affected {changed} rows"),
        )
    }
}

fn require_non_empty(value: &str, message: &str) -> rusqlite::Result<()> {
    if value.is_empty() {
        Err(DbWriteQueue::sync_contract_error(message))
    } else {
        Ok(())
    }
}

fn require_changed(changed: usize, message: String) -> rusqlite::Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(message))
    }
}

fn replace_group_members(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    dto: &GroupSyncDTO,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM group_members WHERE group_id = ?", [id])?;
    let member_tags = dto.member_tags.as_ref().and_then(|value| value.as_object());
    for member in &dto.members {
        let tag = member_tags
            .and_then(|tags| tags.get(member))
            .and_then(|value| value.as_str());
        tx.execute(
            "INSERT INTO group_members
             (group_id, agent_id, member_tag, sort_order, updated_at)
             VALUES (?, ?, ?, 0, ?)",
            rusqlite::params![id, member, tag, now],
        )?;
    }
    Ok(())
}

fn is_valid_avatar_owner(owner_type: &str, owner_id: &str) -> bool {
    match owner_type {
        "agent" | "group" => !owner_id.is_empty(),
        "user" => owner_id == "user_avatar",
        _ => false,
    }
}

fn live_parent(
    tx: &rusqlite::Transaction<'_>,
    owner_type: &str,
    owner_id: &str,
) -> rusqlite::Result<bool> {
    match owner_type {
        "agent" => exists_live(tx, "agents", "agent_id", owner_id),
        "group" => exists_live(tx, "groups", "group_id", owner_id),
        "user" => Ok(true),
        _ => Ok(false),
    }
}

fn exists_live(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    id: &str,
) -> rusqlite::Result<bool> {
    let sql =
        format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} = ? AND deleted_at IS NULL)");
    tx.query_row(&sql, [id], |row| row.get::<_, bool>(0))
}

fn validate_topic_input(
    topic_id: &str,
    dto_id: &str,
    owner_id: &str,
    label: &str,
) -> rusqlite::Result<()> {
    if topic_id.is_empty() || topic_id != dto_id || owner_id.is_empty() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} requires matching non-empty topic and owner ids"
        )));
    }
    Ok(())
}

fn validate_live_owner(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    owner_id: &str,
    label: &str,
    topic_id: &str,
) -> rusqlite::Result<()> {
    let column = if table == "agents" {
        "agent_id"
    } else {
        "group_id"
    };
    if !exists_live(tx, table, column, owner_id)? {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} {topic_id} owner {owner_id} is missing or deleted"
        )));
    }
    Ok(())
}

fn validate_existing_topic(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    owner_id: &str,
    owner_type: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let existing = tx
        .query_row(
            "SELECT owner_id, owner_type, deleted_at FROM topics WHERE topic_id = ?",
            [topic_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((existing_owner, existing_type, deleted_at)) = existing else {
        return Ok(());
    };
    if deleted_at.is_some() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} {topic_id} is tombstoned"
        )));
    }
    if existing_owner != owner_id || existing_type != owner_type {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} {topic_id} owner conflicts with the existing live topic"
        )));
    }
    Ok(())
}
