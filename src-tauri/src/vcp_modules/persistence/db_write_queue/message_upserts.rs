use super::message_batch::{
    filter_canonical_payload, filter_legacy_payload, find_tombstoned_dto_ids, find_tombstoned_ids,
    validate_topic_key,
};
use super::{DbWriteQueue, ExpectedMessageStates, SNAPSHOT_STALE_MARKER};

use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::sync_types::{MessageDeletedState, MessageLiveState, MessageVersionState};
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::OptionalExtension;
use std::collections::HashSet;

#[path = "message_upsert_rows.rs"]
mod message_upsert_rows;

impl DbWriteQueue {
    /// Compatibility facade for old queue callers. The topic-only input is
    /// resolved to one live composite topic and ambiguous ids fail closed.
    pub(super) fn rusqlite_upsert_messages_batch(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
        messages: Vec<ChatMessage>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
        _content_hashes: Vec<String>,
    ) -> rusqlite::Result<()> {
        let key = Self::rusqlite_resolve_topic_key(tx, topic_id)?;
        validate_legacy_message_batch(tx, &key, &messages, &compressed_contents, &render_bytes)?;
        let tombstoned_ids = find_tombstoned_ids(tx, &key, &messages)?;
        let (messages, compressed_contents, render_bytes) =
            filter_legacy_payload(messages, compressed_contents, render_bytes, &tombstoned_ids);
        if messages.is_empty() {
            return Ok(());
        }

        let canonical_messages = messages
            .iter()
            .map(|message| {
                MessageSyncDTO::from_message(
                    message,
                    message.updated_at.unwrap_or(message.timestamp),
                )
                .map_err(DbWriteQueue::sync_contract_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::upsert_canonical_rows(
            tx,
            &key,
            &messages,
            &canonical_messages,
            &compressed_contents,
            &render_bytes,
        )
    }

    /// Canonical Wire 1.4 queue write. Hashes are derived from the DTO and
    /// cannot be supplied by a local-only caller.
    pub(super) fn rusqlite_upsert_messages_batch_for_key(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        messages: Vec<MessageSyncDTO>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
    ) -> rusqlite::Result<()> {
        Self::rusqlite_upsert_messages_batch_for_key_if_unchanged(
            tx,
            key,
            messages,
            compressed_contents,
            render_bytes,
            None,
        )
    }

    pub(super) fn rusqlite_upsert_messages_batch_for_key_if_unchanged(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        messages: Vec<MessageSyncDTO>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
        expected_states: Option<&ExpectedMessageStates>,
    ) -> rusqlite::Result<()> {
        validate_canonical_message_batch(tx, key, &messages, &compressed_contents, &render_bytes)?;
        if let Some(expected) = expected_states {
            validate_message_snapshot(tx, key, &messages, expected)?;
        }
        let tombstoned_ids = find_tombstoned_dto_ids(tx, key, &messages)?;
        let (messages, compressed_contents, render_bytes) =
            filter_canonical_payload(messages, compressed_contents, render_bytes, &tombstoned_ids);
        if messages.is_empty() {
            return Ok(());
        }
        Self::upsert_canonical_rows(tx, key, &[], &messages, &compressed_contents, &render_bytes)
    }

    fn upsert_canonical_rows(
        tx: &rusqlite::Transaction<'_>,
        key: &TopicKey,
        local_messages: &[ChatMessage],
        messages: &[MessageSyncDTO],
        compressed_contents: &[Vec<u8>],
        render_bytes: &[Vec<u8>],
    ) -> rusqlite::Result<()> {
        message_upsert_rows::insert_message_rows(
            tx,
            key,
            messages,
            compressed_contents,
            render_bytes,
        )?;
        super::message_fts::refresh_fts(tx, key, messages)?;
        if local_messages.is_empty() {
            super::attachments::write_attachments_for_dto(tx, key, messages)
        } else {
            super::attachments::write_attachments(tx, key, local_messages, messages)
        }
    }
}

fn validate_message_snapshot(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
    expected: &ExpectedMessageStates,
) -> rusqlite::Result<()> {
    if expected.len() != messages.len()
        || messages
            .iter()
            .any(|message| !expected.contains_key(&message.id))
    {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "{SNAPSHOT_STALE_MARKER}: pull snapshot does not cover the canonical message batch"
        )));
    }
    for message in messages {
        let current = load_message_version(tx, key, &message.id)?;
        if current != expected[&message.id] {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "{SNAPSHOT_STALE_MARKER}: local message changed after the Phase 3 snapshot"
            )));
        }
    }
    Ok(())
}

fn load_message_version(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    message_id: &str,
) -> rusqlite::Result<Option<MessageVersionState>> {
    tx.query_row(
        "SELECT content_hash, updated_at, deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id, message_id],
        |row| {
            let deleted_at = row.get::<_, Option<i64>>(2)?;
            Ok(match deleted_at {
                Some(deleted_at) => {
                    MessageVersionState::Deleted(MessageDeletedState { deleted_at })
                }
                None => MessageVersionState::Live(MessageLiveState {
                    message_hash: row.get(0)?,
                    updated_at: row.get(1)?,
                }),
            })
        },
    )
    .optional()
}

fn validate_legacy_message_batch(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[ChatMessage],
    compressed_contents: &[Vec<u8>],
    render_bytes: &[Vec<u8>],
) -> rusqlite::Result<()> {
    validate_topic_key(key)?;
    if messages.len() != compressed_contents.len() || messages.len() != render_bytes.len() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {} message batch vectors have inconsistent lengths",
            key.topic_id
        )));
    }
    validate_live_parent(tx, key)?;
    let mut ids = HashSet::with_capacity(messages.len());
    for message in messages {
        if message.id.is_empty() || message.role.is_empty() {
            return Err(DbWriteQueue::sync_contract_error(
                "Message batch requires non-empty message ids and roles",
            ));
        }
        if !ids.insert(message.id.clone()) {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Message batch contains duplicate id {}",
                message.id
            )));
        }
        if message
            .topic_id
            .as_deref()
            .is_some_and(|message_topic| message_topic != key.topic_id)
        {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Message batch contains a topic id conflicting with {}",
                key.topic_id
            )));
        }
    }
    Ok(())
}

fn validate_canonical_message_batch(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
    compressed_contents: &[Vec<u8>],
    render_bytes: &[Vec<u8>],
) -> rusqlite::Result<()> {
    validate_topic_key(key)?;
    if messages.len() != compressed_contents.len() || messages.len() != render_bytes.len() {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {} canonical message vectors have inconsistent lengths",
            key.topic_id
        )));
    }
    validate_live_parent(tx, key)?;
    let mut ids = HashSet::with_capacity(messages.len());
    for message in messages {
        if message.id.is_empty() || message.role.is_empty() {
            return Err(DbWriteQueue::sync_contract_error(
                "Canonical message requires non-empty id and role",
            ));
        }
        if !ids.insert(message.id.clone()) {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Canonical message batch contains duplicate id {}",
                message.id
            )));
        }
        if message
            .topic_id
            .as_deref()
            .is_some_and(|message_topic| message_topic != key.topic_id)
        {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Canonical message {} has a topic id conflicting with {}",
                message.id, key.topic_id
            )));
        }
        for attachment in message.attachments.as_deref().unwrap_or_default() {
            if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&attachment.hash) {
                return Err(DbWriteQueue::sync_contract_error(format!(
                    "Canonical message {} attachment {} has an invalid SHA-256 hash",
                    message.id, attachment.name
                )));
            }
            if attachment.attachment_order.is_some_and(|order| order < 0) {
                return Err(DbWriteQueue::sync_contract_error(format!(
                    "Canonical message {} attachment {} has a negative attachment order",
                    message.id, attachment.name
                )));
            }
        }
    }
    Ok(())
}

fn validate_live_parent(tx: &rusqlite::Transaction<'_>, key: &TopicKey) -> rusqlite::Result<()> {
    let live = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
         )",
        rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id],
        |row| row.get::<_, bool>(0),
    )?;
    if live {
        let (table, id_column, label) = match key.owner_type.as_str() {
            "agent" => ("agents", "agent_id", "Agent"),
            "group" => ("groups", "group_id", "Group"),
            _ => {
                return Err(DbWriteQueue::sync_contract_error(format!(
                    "Message batch topic {} has unsupported owner type {}",
                    key.topic_id, key.owner_type
                )))
            }
        };
        let owner_live = tx.query_row(
            &format!(
                "SELECT EXISTS(
                     SELECT 1 FROM {table}
                     WHERE {id_column} = ? AND deleted_at IS NULL
                 )"
            ),
            [&key.owner_id],
            |row| row.get::<_, bool>(0),
        )?;
        if owner_live {
            Ok(())
        } else {
            Err(DbWriteQueue::sync_contract_error(format!(
                "Message batch {label} owner {} is missing or deleted",
                key.owner_id
            )))
        }
    } else {
        Err(DbWriteQueue::sync_contract_error(format!(
            "Message batch parent topic {}/{} is missing or deleted",
            key.owner_id, key.topic_id
        )))
    }
}
