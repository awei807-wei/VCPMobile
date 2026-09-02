use super::DbWriteQueue;

use crate::vcp_modules::chat_manager::ChatMessage;
use rusqlite::ToSql;
use std::collections::HashSet;

type MessagePayload = (Vec<ChatMessage>, Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<String>);

struct MessageInsertContext<'a, 'tx> {
    tx: &'a rusqlite::Transaction<'tx>,
    topic_id: &'a str,
    messages: &'a [ChatMessage],
    compressed_contents: &'a [Vec<u8>],
    render_bytes: &'a [Vec<u8>],
    content_hashes: &'a [String],
    now: i64,
}

impl DbWriteQueue {
    pub(super) fn rusqlite_upsert_messages_batch(
        tx: &rusqlite::Transaction<'_>,
        topic_id: &str,
        messages: Vec<ChatMessage>,
        compressed_contents: Vec<Vec<u8>>,
        render_bytes: Vec<Vec<u8>>,
        content_hashes: Vec<String>,
    ) -> rusqlite::Result<()> {
        validate_message_batch(
            tx,
            topic_id,
            &messages,
            &compressed_contents,
            &render_bytes,
            &content_hashes,
        )?;
        if messages.is_empty() {
            return Ok(());
        }
        let tombstoned_ids = find_tombstoned_ids(tx, topic_id, &messages)?;
        let (messages, compressed_contents, render_bytes, content_hashes) =
            filter_tombstoned_payload(
                messages,
                compressed_contents,
                render_bytes,
                content_hashes,
                &tombstoned_ids,
            );
        if messages.is_empty() {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        insert_message_rows(MessageInsertContext {
            tx,
            topic_id,
            messages: &messages,
            compressed_contents: &compressed_contents,
            render_bytes: &render_bytes,
            content_hashes: &content_hashes,
            now,
        })?;
        refresh_fts(tx, topic_id, &messages)?;
        write_attachments(tx, topic_id, &messages)
    }
}

fn validate_message_batch(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
    compressed_contents: &[Vec<u8>],
    render_bytes: &[Vec<u8>],
    content_hashes: &[String],
) -> rusqlite::Result<()> {
    if topic_id.is_empty() {
        return Err(DbWriteQueue::sync_contract_error(
            "Message batch requires a non-empty topic id",
        ));
    }
    if messages.len() != compressed_contents.len()
        || messages.len() != render_bytes.len()
        || messages.len() != content_hashes.len()
    {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Topic {topic_id} message batch vectors have inconsistent lengths"
        )));
    }
    let live = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM topics WHERE topic_id = ? AND deleted_at IS NULL)",
        [topic_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !live {
        return Err(DbWriteQueue::sync_contract_error(format!(
            "Message batch parent topic {topic_id} is missing or deleted"
        )));
    }
    let mut ids = HashSet::with_capacity(messages.len());
    for message in messages {
        if message.id.is_empty() {
            return Err(DbWriteQueue::sync_contract_error(
                "Message batch requires non-empty message ids",
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
            .is_some_and(|message_topic| message_topic != topic_id)
        {
            return Err(DbWriteQueue::sync_contract_error(format!(
                "Message batch contains a topic id conflicting with {topic_id}"
            )));
        }
    }
    Ok(())
}

fn find_tombstoned_ids(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
) -> rusqlite::Result<HashSet<String>> {
    let mut tombstoned = HashSet::new();
    for chunk in messages.chunks(998) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT msg_id FROM messages
             WHERE topic_id = ? AND deleted_at IS NOT NULL AND msg_id IN ({placeholders})"
        );
        let mut params = Vec::with_capacity(chunk.len() + 1);
        params.push(topic_id.to_string());
        params.extend(chunk.iter().map(|message| message.id.clone()));
        let mut statement = tx.prepare_cached(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(params), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            tombstoned.insert(row?);
        }
    }
    Ok(tombstoned)
}

fn filter_tombstoned_payload(
    messages: Vec<ChatMessage>,
    compressed_contents: Vec<Vec<u8>>,
    render_bytes: Vec<Vec<u8>>,
    content_hashes: Vec<String>,
    tombstoned_ids: &HashSet<String>,
) -> MessagePayload {
    if tombstoned_ids.is_empty() {
        return (messages, compressed_contents, render_bytes, content_hashes);
    }
    let kept = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (!tombstoned_ids.contains(&message.id)).then_some(index))
        .collect::<Vec<_>>();
    let filtered_messages = kept.iter().map(|index| messages[*index].clone()).collect();
    let filtered_contents = kept
        .iter()
        .map(|index| compressed_contents[*index].clone())
        .collect();
    let filtered_render = kept
        .iter()
        .map(|index| render_bytes[*index].clone())
        .collect();
    let filtered_hashes = kept
        .iter()
        .map(|index| content_hashes[*index].clone())
        .collect();
    (
        filtered_messages,
        filtered_contents,
        filtered_render,
        filtered_hashes,
    )
}

fn insert_message_rows(context: MessageInsertContext<'_, '_>) -> rusqlite::Result<()> {
    const MAX_PARAMS: usize = 999;
    const PARAMS_PER_MESSAGE: usize = 13;
    let indices = (0..context.messages.len()).collect::<Vec<_>>();
    for chunk in indices.chunks(MAX_PARAMS / PARAMS_PER_MESSAGE) {
        insert_message_chunk(&context, chunk)?;
    }
    Ok(())
}

fn insert_message_chunk(
    context: &MessageInsertContext<'_, '_>,
    indices: &[usize],
) -> rusqlite::Result<()> {
    let values = vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; indices.len()].join(", ");
    let sql = format!(
        "INSERT INTO messages (
            msg_id, topic_id, role, name, agent_id, content, timestamp,
            is_group_message, group_id, finish_reason, content_hash, created_at, updated_at
        ) VALUES {values}
        ON CONFLICT(topic_id, msg_id) DO UPDATE SET
            content = excluded.content, role = excluded.role, name = excluded.name,
            agent_id = excluded.agent_id, is_group_message = excluded.is_group_message,
            group_id = excluded.group_id, finish_reason = excluded.finish_reason,
            content_hash = excluded.content_hash, updated_at = excluded.updated_at,
            deleted_at = NULL"
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(indices.len() * 13);
    for index in indices {
        let message = &context.messages[*index];
        params.extend([
            Box::new(message.id.clone()) as Box<dyn ToSql>,
            Box::new(context.topic_id.to_string()),
            Box::new(message.role.clone()),
            Box::new(message.name.clone()),
            Box::new(message.agent_id.clone()),
            Box::new(context.compressed_contents[*index].clone()),
            Box::new(message.timestamp as i64),
            Box::new(message.is_group_message.unwrap_or(false)),
            Box::new(message.group_id.clone()),
            Box::new(message.finish_reason.clone()),
            Box::new(context.content_hashes[*index].clone()),
            Box::new(message.timestamp as i64),
            Box::new(context.now),
        ]);
    }
    let refs = params
        .iter()
        .map(|param| param.as_ref())
        .collect::<Vec<_>>();
    context.tx.execute(&sql, &*refs)?;
    insert_render_rows(
        context.tx,
        context.topic_id,
        indices,
        context.messages,
        context.render_bytes,
        context.now,
    )
}

fn insert_render_rows(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    indices: &[usize],
    messages: &[ChatMessage],
    render_bytes: &[Vec<u8>],
    now: i64,
) -> rusqlite::Result<()> {
    let indices = indices
        .iter()
        .copied()
        .filter(|index| !render_bytes[*index].is_empty())
        .collect::<Vec<_>>();
    if indices.is_empty() {
        return Ok(());
    }
    let values = vec!["(?, ?, ?, ?)"; indices.len()].join(", ");
    let sql = format!(
        "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at)
         VALUES {values}
         ON CONFLICT(topic_id, msg_id) DO UPDATE SET
            render_content = excluded.render_content, updated_at = excluded.updated_at"
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(indices.len() * 4);
    for index in indices {
        params.extend([
            Box::new(topic_id.to_string()) as Box<dyn ToSql>,
            Box::new(messages[index].id.clone()),
            Box::new(render_bytes[index].clone()),
            Box::new(now),
        ]);
    }
    let refs = params
        .iter()
        .map(|param| param.as_ref())
        .collect::<Vec<_>>();
    tx.execute(&sql, &*refs)?;
    Ok(())
}

fn refresh_fts(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
) -> rusqlite::Result<()> {
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    for chunk in ids.chunks(998) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql =
            format!("DELETE FROM messages_fts WHERE topic_id = ? AND msg_id IN ({placeholders})");
        let mut params = Vec::with_capacity(chunk.len() + 1);
        params.push(topic_id.to_string());
        params.extend(chunk.iter().cloned());
        tx.execute(&sql, rusqlite::params_from_iter(params))?;
    }
    for chunk in messages.chunks(333) {
        insert_fts_chunk(tx, topic_id, chunk)?;
    }
    Ok(())
}

fn insert_fts_chunk(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
) -> rusqlite::Result<()> {
    let values = vec!["(?, ?, ?)"; messages.len()].join(", ");
    let sql = format!("INSERT INTO messages_fts (msg_id, topic_id, content) VALUES {values}");
    let mut params = Vec::with_capacity(messages.len() * 3);
    for message in messages {
        params.push(message.id.clone());
        params.push(topic_id.to_string());
        params.push(message.content.clone());
    }
    tx.execute(&sql, rusqlite::params_from_iter(params))?;
    Ok(())
}

fn write_attachments(
    tx: &rusqlite::Transaction<'_>,
    topic_id: &str,
    messages: &[ChatMessage],
) -> rusqlite::Result<()> {
    super::attachments::write_attachments(tx, topic_id, messages)
}
