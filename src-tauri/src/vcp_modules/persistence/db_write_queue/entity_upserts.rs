use super::DbWriteQueue;

use crate::vcp_modules::sync_dto::{AgentSyncDTO, AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_error::SyncErrorStage;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::OptionalExtension;

impl DbWriteQueue {
    pub(super) fn rusqlite_upsert_agent(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &AgentSyncDTO,
    ) -> rusqlite::Result<()> {
        Self::rusqlite_upsert_agent_if_expected(tx, id, dto, None).map(|_| ())
    }

    pub(super) fn rusqlite_upsert_agent_if_expected(
        tx: &rusqlite::Transaction<'_>,
        id: &str,
        dto: &AgentSyncDTO,
        expected_config_hash: Option<&str>,
    ) -> rusqlite::Result<()> {
        require_non_empty(id, "Agent upsert requires a non-empty id")?;
        if !expected_config_matches(tx, "agents", "agent_id", id, expected_config_hash)? {
            log::warn!(
                "[DbWriteQueue] skipped stale Agent pull for {id}: local config_hash changed"
            );
            return Err(DbWriteQueue::sync_snapshot_stale_error(
                format!("local Agent {id} changed after the sync snapshot"),
                SyncErrorStage::OwnerMetadata,
            ));
        }
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
        require_changed(changed, format!("Agent {id} is tombstoned"))?;
        Ok(())
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
        let key = TopicKey::new("agent", dto.owner_id.clone(), topic_id);
        Self::rusqlite_upsert_agent_topic_for_key(tx, &key, dto)
    }

    pub(super) fn rusqlite_upsert_agent_topic_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        dto: &AgentTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        validate_topic_key(key, "Agent topic")?;
        if key.owner_type != "agent" {
            return Err(DbWriteQueue::sync_contract_error(
                "Agent topic requires ownerType=agent",
            ));
        }
        validate_topic_input(
            &key.topic_id,
            &dto.id,
            &dto.owner_id,
            &key.owner_id,
            "Agent topic",
        )?;
        validate_live_owner(tx, "agents", &key.owner_id, "Agent topic", &key.topic_id)?;
        validate_existing_topic(tx, key, "Agent topic")?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = tx.execute(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, locked, unread, updated_at
            )
            VALUES ('agent', ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
                title = excluded.title, locked = excluded.locked,
                unread = excluded.unread,
                unread_count = CASE WHEN excluded.unread = 0 THEN 0 ELSE topics.unread_count END,
                updated_at = excluded.updated_at",
            rusqlite::params![
                &key.owner_id,
                &key.topic_id,
                &dto.name,
                dto.created_at,
                if dto.locked { 1 } else { 0 },
                if dto.unread { 1 } else { 0 },
                now,
            ],
        )?;
        require_changed(
            changed,
            format!(
                "Agent topic {}/{} upsert affected {changed} rows",
                key.owner_id, key.topic_id
            ),
        )?;
        if !dto.unread {
            clear_topic_unread_receipts(tx, key)?;
        }
        Ok(())
    }

    pub(super) fn rusqlite_upsert_group_topic(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
        dto: &GroupTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        let key = TopicKey::new("group", dto.owner_id.clone(), topic_id);
        Self::rusqlite_upsert_group_topic_for_key(tx, &key, dto)
    }

    pub(super) fn rusqlite_upsert_group_topic_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        dto: &GroupTopicSyncDTO,
    ) -> rusqlite::Result<()> {
        validate_topic_key(key, "Group topic")?;
        if key.owner_type != "group" {
            return Err(DbWriteQueue::sync_contract_error(
                "Group topic requires ownerType=group",
            ));
        }
        validate_topic_input(
            &key.topic_id,
            &dto.id,
            &dto.owner_id,
            &key.owner_id,
            "Group topic",
        )?;
        validate_live_owner(tx, "groups", &key.owner_id, "Group topic", &key.topic_id)?;
        validate_existing_topic(tx, key, "Group topic")?;
        let now = chrono::Utc::now().timestamp_millis();
        let changed = tx.execute(
            "INSERT INTO topics (
                owner_type, owner_id, topic_id, title, created_at, locked, unread, updated_at
            )
            VALUES ('group', ?, ?, ?, ?, 1, 0, ?)
            ON CONFLICT(owner_type, owner_id, topic_id) DO UPDATE SET
                title = excluded.title, updated_at = excluded.updated_at",
            rusqlite::params![&key.owner_id, &key.topic_id, &dto.name, dto.created_at, now,],
        )?;
        require_changed(
            changed,
            format!(
                "Group topic {}/{} upsert affected {changed} rows",
                key.owner_id, key.topic_id
            ),
        )
    }
}

fn clear_topic_unread_receipts(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
) -> rusqlite::Result<()> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'message_unread_receipts'
            )",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    tx.execute(
        "UPDATE message_unread_receipts
         SET counted_unread = 0
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1",
        rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
    )?;
    Ok(())
}

pub(super) fn require_non_empty(value: &str, message: &str) -> rusqlite::Result<()> {
    if value.is_empty() {
        Err(DbWriteQueue::sync_contract_error(message))
    } else {
        Ok(())
    }
}

pub(super) fn require_changed(changed: usize, message: String) -> rusqlite::Result<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(message))
    }
}

pub(super) fn expected_config_matches(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    id_column: &str,
    id: &str,
    expected_config_hash: Option<&str>,
) -> rusqlite::Result<bool> {
    let Some(expected_config_hash) = expected_config_hash else {
        return Ok(true);
    };
    let sql =
        format!("SELECT config_hash FROM {table} WHERE {id_column} = ? AND deleted_at IS NULL");
    let current = tx
        .query_row(&sql, [id], |row| row.get::<_, String>(0))
        .optional()?;
    Ok(current.is_some_and(|current| current == expected_config_hash))
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

fn validate_topic_key(key: &TopicKey, label: &str) -> rusqlite::Result<()> {
    if !key.is_valid() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} requires a valid composite topic identity"
        )));
    }
    Ok(())
}

fn validate_topic_input(
    topic_id: &str,
    dto_id: &str,
    owner_id: &str,
    key_owner_id: &str,
    label: &str,
) -> rusqlite::Result<()> {
    if topic_id.is_empty() || topic_id != dto_id || owner_id.is_empty() || owner_id != key_owner_id
    {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} requires matching non-empty composite topic identity"
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
    key: &TopicKey,
    label: &str,
) -> rusqlite::Result<()> {
    let existing = tx
        .query_row(
            "SELECT deleted_at FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
            rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?;
    let Some(deleted_at) = existing else {
        return Ok(());
    };
    if deleted_at.is_some() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{label} {}/{} is tombstoned",
            key.owner_id, key.topic_id
        )));
    }
    Ok(())
}
