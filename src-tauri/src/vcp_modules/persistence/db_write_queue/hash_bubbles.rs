use super::DbWriteQueue;

use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_hash::HashAggregator;
use rusqlite::OptionalExtension;

impl DbWriteQueue {
    pub(super) fn rusqlite_bubble_topic_hash(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
    ) -> rusqlite::Result<()> {
        require_non_empty(topic_id, "Topic hash bubble requires a non-empty topic id")?;
        let (owner_id, owner_type) = load_live_topic_owner(tx, topic_id)?;
        let content_hash = load_topic_content_hash(tx, topic_id)?;
        let config_hash = topic_config_hash(tx, topic_id, &owner_type)?;
        tx.execute(
            "UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ?",
            rusqlite::params![content_hash, config_hash, topic_id],
        )?;
        let _ = owner_id;
        Ok(())
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

fn load_live_topic_owner(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
) -> rusqlite::Result<(String, String)> {
    let topic = tx
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
        .optional()?
        .ok_or_else(|| DbWriteQueue::sync_contract_error(format!("Topic {topic_id} is missing")))?;
    if topic.2.is_some() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {topic_id} is tombstoned"
        )));
    }
    let column = match topic.1.as_str() {
        "agent" => "agent_id",
        "group" => "group_id",
        _ => {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Topic {topic_id} has unsupported owner type {}",
                topic.1
            )))
        }
    };
    validate_live_owner(tx, owner_table(&topic.1), column, &topic.0, "Topic")?;
    Ok((topic.0, topic.1))
}

fn owner_table(owner_type: &str) -> &'static str {
    match owner_type {
        "agent" => "agents",
        "group" => "groups",
        _ => "",
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
    topic_id: &str,
) -> rusqlite::Result<String> {
    let mut statement = tx.prepare(
        "SELECT content_hash FROM messages
         WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp ASC, msg_id ASC",
    )?;
    let hashes = statement
        .query_map([topic_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(crate::vcp_modules::sync_types::compute_merkle_root(hashes))
}

fn topic_config_hash(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    owner_type: &str,
) -> rusqlite::Result<String> {
    match owner_type {
        "agent" => {
            let dto = DbWriteQueue::rusqlite_load_agent_topic_dto(tx, topic_id)?;
            Ok(HashAggregator::compute_agent_topic_metadata_hash(&dto))
        }
        "group" => {
            let dto = DbWriteQueue::rusqlite_load_group_topic_dto(tx, topic_id)?;
            Ok(HashAggregator::compute_group_topic_metadata_hash(&dto))
        }
        _ => Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {topic_id} has unsupported owner type {owner_type}"
        ))),
    }
}

fn load_owner_topics_hash(
    tx: &rusqlite::Transaction<'_>,
    owner_id: &str,
    owner_type: &str,
) -> rusqlite::Result<String> {
    let mut statement = tx.prepare(
        "SELECT config_hash, content_hash FROM topics
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL ORDER BY topic_id ASC",
    )?;
    let mut rows = statement.query(rusqlite::params![owner_id, owner_type])?;
    let mut hashes = Vec::new();
    while let Some(row) = rows.next()? {
        hashes.push(row.get::<_, String>(0)?);
        hashes.push(row.get::<_, String>(1)?);
    }
    Ok(crate::vcp_modules::sync_types::compute_merkle_root(hashes))
}

fn update_owner_hash(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    owner_id: &str,
    root_hash: &str,
    label: &str,
) -> rusqlite::Result<()> {
    let sql = format!("UPDATE {table} SET content_hash = ? WHERE {column} = ?");
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
        tx.query_row(
            "SELECT topic_id, title, created_at, locked, unread, owner_id
             FROM topics WHERE topic_id = ?",
            [topic_id],
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
        tx.query_row(
            "SELECT topic_id, title, created_at, owner_id
             FROM topics WHERE topic_id = ?",
            [topic_id],
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
