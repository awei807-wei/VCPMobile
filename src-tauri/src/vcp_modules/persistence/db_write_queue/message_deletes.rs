use super::message_batch::validate_topic_key;
use super::DbWriteQueue;

#[path = "message_delete_unread.rs"]
mod unread_receipts;

use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use rusqlite::OptionalExtension;
use rusqlite::ToSql;

impl DbWriteQueue {
    /// Tombstone one message under a complete Wire 1.4 message identity.
    /// Returns `true` only when a live row was changed.
    pub(super) fn rusqlite_delete_message_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &MessageKey,
        deleted_at: i64,
    ) -> rusqlite::Result<bool> {
        validate_message_key(key)?;
        validate_deleted_at(deleted_at)?;
        let current = tx
            .query_row(
                "SELECT deleted_at FROM messages
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
                rusqlite::params![
                    &key.topic.owner_type,
                    &key.topic.owner_id,
                    &key.topic.topic_id,
                    &key.msg_id
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        if current != Some(None) {
            return Ok(false);
        }

        delete_message_side_tables(tx, &key.topic, std::slice::from_ref(&key.msg_id))?;
        let changed = tx.execute(
            "UPDATE messages SET deleted_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
               AND deleted_at IS NULL",
            rusqlite::params![
                deleted_at,
                &key.topic.owner_type,
                &key.topic.owner_id,
                &key.topic.topic_id,
                &key.msg_id
            ],
        )?;
        Ok(changed == 1)
    }

    /// Tombstone a complete topic namespace and all of its live messages.
    /// Returns `true` only when a live topic was changed.
    pub(super) fn rusqlite_delete_topic_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        deleted_at: i64,
    ) -> rusqlite::Result<bool> {
        validate_topic_key(key)?;
        validate_deleted_at(deleted_at)?;
        let current = tx
            .query_row(
                "SELECT deleted_at FROM topics
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
                rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        if current != Some(None) {
            return Ok(false);
        }

        let ids = load_message_ids(tx, key)?;
        delete_message_side_tables(tx, key, &ids)?;
        tx.execute(
            "UPDATE messages SET deleted_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL",
            rusqlite::params![deleted_at, &key.owner_type, &key.owner_id, &key.topic_id],
        )?;
        let changed = tx.execute(
            "UPDATE topics SET deleted_at = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL",
            rusqlite::params![deleted_at, &key.owner_type, &key.owner_id, &key.topic_id],
        )?;
        Ok(changed == 1)
    }
}

fn validate_message_key(key: &MessageKey) -> rusqlite::Result<()> {
    if key.is_valid() {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(
            "Message operation requires a valid composite message identity",
        ))
    }
}

fn validate_deleted_at(deleted_at: i64) -> rusqlite::Result<()> {
    if deleted_at >= 0 {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(
            "Delete timestamp must be non-negative",
        ))
    }
}

fn load_message_ids(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = tx.prepare(
        "SELECT msg_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )?;
    let result = statement
        .query_map(
            rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
            |row| row.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>();
    result
}

fn delete_message_side_tables(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for chunk in message_ids.chunks(998) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        for table in [
            "messages_fts",
            "render_cache",
            "message_attachments",
            "active_generations",
        ] {
            let sql = format!(
                "DELETE FROM {table}
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
                   AND msg_id IN ({placeholders})"
            );
            let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + 3);
            params.extend([
                Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
                Box::new(key.owner_id.clone()),
                Box::new(key.topic_id.clone()),
            ]);
            params.extend(
                chunk
                    .iter()
                    .cloned()
                    .map(|id| Box::new(id) as Box<dyn ToSql>),
            );
            let refs = params
                .iter()
                .map(|param| param.as_ref())
                .collect::<Vec<_>>();
            tx.execute(&sql, &*refs)?;
        }
        delete_message_unread_receipts(tx, key, chunk)?;
    }
    Ok(())
}

fn delete_message_unread_receipts(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    unread_receipts::delete_message_unread_receipts(tx, key, message_ids)
}
