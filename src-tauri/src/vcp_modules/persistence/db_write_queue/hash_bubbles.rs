use super::DbWriteQueue;

use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::compute_merkle_root;
use crate::vcp_modules::topic_types::TopicKey;

impl DbWriteQueue {
    pub(super) fn rusqlite_bubble_topic_hash(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
    ) -> rusqlite::Result<()> {
        let key = Self::rusqlite_resolve_topic_key(tx, topic_id)?;
        Self::rusqlite_bubble_topic_hash_for_key(tx, &key)
    }

    pub(super) fn rusqlite_bubble_topic_hash_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
    ) -> rusqlite::Result<()> {
        validate_topic_key(key)?;
        validate_live_topic(tx, key)?;
        let content_hash = load_topic_content_hash(tx, key)?;
        let config_hash = topic_config_hash(tx, key)?;
        let changed = tx.execute(
            "UPDATE topics SET content_hash = ?, config_hash = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL",
            rusqlite::params![
                content_hash,
                config_hash,
                &key.owner_type,
                &key.owner_id,
                &key.topic_id
            ],
        )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(DbWriteQueue::sync_contract_error(format!(
                "Topic {}/{} disappeared during hash update",
                key.owner_id, key.topic_id
            )))
        }
    }

    pub(super) fn rusqlite_bubble_agent_hash(
        tx: &rusqlite::Transaction<'_>,
        agent_id: &str,
    ) -> rusqlite::Result<()> {
        require_non_empty(agent_id, "Agent hash bubble requires a non-empty owner id")?;
        validate_live_owner(tx, "agents", "agent_id", agent_id, "Agent")?;
        let root_hash = load_owner_topics_hash(tx, agent_id, "agent")?;
        update_owner_hash(tx, "agents", "agent_id", agent_id, &root_hash, "Agent")
    }

    pub(super) fn rusqlite_bubble_group_hash(
        tx: &rusqlite::Transaction<'_>,
        group_id: &str,
    ) -> rusqlite::Result<()> {
        require_non_empty(group_id, "Group hash bubble requires a non-empty owner id")?;
        validate_live_owner(tx, "groups", "group_id", group_id, "Group")?;
        let root_hash = load_owner_topics_hash(tx, group_id, "group")?;
        update_owner_hash(tx, "groups", "group_id", group_id, &root_hash, "Group")
    }
}

fn require_non_empty(value: &str, message: &str) -> rusqlite::Result<()> {
    if value.is_empty() {
        Err(DbWriteQueue::sync_contract_error(message))
    } else {
        Ok(())
    }
}

fn validate_topic_key(key: &TopicKey) -> rusqlite::Result<()> {
    if key.is_valid() {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(
            "Topic hash bubble requires a valid composite topic identity",
        ))
    }
}

fn validate_live_topic(tx: &rusqlite::Transaction<'_>, key: &TopicKey) -> rusqlite::Result<()> {
    let live = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         )",
        rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
        |row| row.get::<_, bool>(0),
    )?;
    if live {
        match key.owner_type.as_str() {
            "agent" => validate_live_owner(tx, "agents", "agent_id", &key.owner_id, "Agent"),
            "group" => validate_live_owner(tx, "groups", "group_id", &key.owner_id, "Group"),
            _ => Err(DbWriteQueue::sync_contract_error(format!(
                "Topic {} has unsupported owner type {}",
                key.topic_id, key.owner_type
            ))),
        }
    } else {
        Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {}/{} is missing or deleted",
            key.owner_id, key.topic_id
        )))
    }
}

fn validate_live_owner(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    owner_id: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let sql =
        format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} = ? AND deleted_at IS NULL)");
    let live = tx.query_row(&sql, [owner_id], |row| row.get::<_, bool>(0))?;
    if live {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(format!(
            "{label} owner {owner_id} is missing or deleted"
        )))
    }
}

fn load_topic_content_hash(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
) -> rusqlite::Result<String> {
    let mut statement = tx.prepare(
        "SELECT msg_id, content_hash FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         ORDER BY timestamp ASC, msg_id ASC",
    )?;
    let rows = statement.query_map(
        rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut leaves = Vec::new();
    for row in rows {
        let (message_id, message_hash) = row?;
        leaves.push(HashAggregator::compute_message_leaf_hash(
            &message_id,
            &message_hash,
        ));
    }
    Ok(compute_merkle_root(leaves))
}

fn topic_config_hash(tx: &rusqlite::Transaction<'_>, key: &TopicKey) -> rusqlite::Result<String> {
    match key.owner_type.as_str() {
        "agent" => {
            let dto = DbWriteQueue::rusqlite_load_agent_topic_dto_for_key(tx, key)?;
            Ok(HashAggregator::compute_agent_topic_metadata_hash(&dto))
        }
        "group" => {
            let dto = DbWriteQueue::rusqlite_load_group_topic_dto_for_key(tx, key)?;
            Ok(HashAggregator::compute_group_topic_metadata_hash(&dto))
        }
        other => Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {} has unsupported owner type {other}",
            key.topic_id
        ))),
    }
}

fn load_owner_topics_hash(
    tx: &rusqlite::Transaction<'_>,
    owner_id: &str,
    owner_type: &str,
) -> rusqlite::Result<String> {
    let mut statement = tx.prepare(
        "SELECT topic_id, config_hash, content_hash FROM topics
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL
         ORDER BY topic_id ASC",
    )?;
    let rows = statement.query_map(rusqlite::params![owner_id, owner_type], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut leaves = Vec::new();
    for row in rows {
        let (topic_id, config_hash, content_hash) = row?;
        leaves.push(HashAggregator::compute_topic_leaf_hash(
            &topic_id,
            &config_hash,
            &content_hash,
        ));
    }
    Ok(compute_merkle_root(leaves))
}

fn update_owner_hash(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    owner_id: &str,
    root_hash: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let sql =
        format!("UPDATE {table} SET content_hash = ? WHERE {column} = ? AND deleted_at IS NULL");
    let changed = tx.execute(&sql, rusqlite::params![root_hash, owner_id])?;
    if changed == 1 {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(format!(
            "{label} hash bubble owner {owner_id} was not updated"
        )))
    }
}

impl DbWriteQueue {
    pub(super) fn rusqlite_load_agent_topic_dto(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
    ) -> rusqlite::Result<AgentTopicSyncDTO> {
        let key = Self::rusqlite_resolve_topic_key(tx, topic_id)?;
        Self::rusqlite_load_agent_topic_dto_for_key(tx, &key)
    }

    pub(super) fn rusqlite_load_agent_topic_dto_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
    ) -> rusqlite::Result<AgentTopicSyncDTO> {
        validate_topic_key(key)?;
        if key.owner_type != "agent" {
            return Err(DbWriteQueue::sync_contract_error(
                "Agent topic DTO requires ownerType=agent",
            ));
        }
        tx.query_row(
            "SELECT topic_id, title, created_at, locked, unread, owner_id
             FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
            rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
            |row| {
                Ok(AgentTopicSyncDTO {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    locked: row.get::<_, i64>(3)? != 0,
                    unread: row.get::<_, i64>(4)? != 0,
                    owner_id: row.get(5)?,
                })
            },
        )
    }

    pub(super) fn rusqlite_load_group_topic_dto(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
    ) -> rusqlite::Result<GroupTopicSyncDTO> {
        let key = Self::rusqlite_resolve_topic_key(tx, topic_id)?;
        Self::rusqlite_load_group_topic_dto_for_key(tx, &key)
    }

    pub(super) fn rusqlite_load_group_topic_dto_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
    ) -> rusqlite::Result<GroupTopicSyncDTO> {
        validate_topic_key(key)?;
        if key.owner_type != "group" {
            return Err(DbWriteQueue::sync_contract_error(
                "Group topic DTO requires ownerType=group",
            ));
        }
        tx.query_row(
            "SELECT topic_id, title, created_at, owner_id
             FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
            rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
            |row| {
                Ok(GroupTopicSyncDTO {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    owner_id: row.get(3)?,
                })
            },
        )
    }
}
