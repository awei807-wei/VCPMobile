use super::DbWriteQueue;

use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::ToSql;
use std::collections::HashSet;

pub(super) type LegacyMessagePayload = (Vec<ChatMessage>, Vec<Vec<u8>>, Vec<Vec<u8>>);
pub(super) type CanonicalMessagePayload = (Vec<MessageSyncDTO>, Vec<Vec<u8>>, Vec<Vec<u8>>);

pub(super) fn validate_topic_key(key: &TopicKey) -> rusqlite::Result<()> {
    if key.is_valid() {
        Ok(())
    } else {
        Err(DbWriteQueue::sync_contract_error(
            "Message batch requires a valid composite topic identity",
        ))
    }
}

pub(super) fn find_tombstoned_ids(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[ChatMessage],
) -> rusqlite::Result<HashSet<String>> {
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    find_tombstoned_id_strings(tx, key, &ids)
}

pub(super) fn find_tombstoned_dto_ids(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
) -> rusqlite::Result<HashSet<String>> {
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    find_tombstoned_id_strings(tx, key, &ids)
}

fn find_tombstoned_id_strings(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    ids: &[String],
) -> rusqlite::Result<HashSet<String>> {
    let mut tombstoned = HashSet::new();
    for chunk in ids.chunks(998) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT msg_id FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NOT NULL AND msg_id IN ({placeholders})"
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
        let mut statement = tx.prepare_cached(&sql)?;
        let rows = statement.query_map(&*refs, |row| row.get::<_, String>(0))?;
        for row in rows {
            tombstoned.insert(row?);
        }
    }
    Ok(tombstoned)
}

pub(super) fn filter_legacy_payload(
    messages: Vec<ChatMessage>,
    compressed_contents: Vec<Vec<u8>>,
    render_bytes: Vec<Vec<u8>>,
    tombstoned_ids: &HashSet<String>,
) -> LegacyMessagePayload {
    if tombstoned_ids.is_empty() {
        return (messages, compressed_contents, render_bytes);
    }
    let kept = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (!tombstoned_ids.contains(&message.id)).then_some(index))
        .collect::<Vec<_>>();
    (
        kept.iter().map(|index| messages[*index].clone()).collect(),
        kept.iter()
            .map(|index| compressed_contents[*index].clone())
            .collect(),
        kept.iter()
            .map(|index| render_bytes[*index].clone())
            .collect(),
    )
}

pub(super) fn filter_canonical_payload(
    messages: Vec<MessageSyncDTO>,
    compressed_contents: Vec<Vec<u8>>,
    render_bytes: Vec<Vec<u8>>,
    tombstoned_ids: &HashSet<String>,
) -> CanonicalMessagePayload {
    if tombstoned_ids.is_empty() {
        return (messages, compressed_contents, render_bytes);
    }
    let kept = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (!tombstoned_ids.contains(&message.id)).then_some(index))
        .collect::<Vec<_>>();
    (
        kept.iter().map(|index| messages[*index].clone()).collect(),
        kept.iter()
            .map(|index| compressed_contents[*index].clone())
            .collect(),
        kept.iter()
            .map(|index| render_bytes[*index].clone())
            .collect(),
    )
}
