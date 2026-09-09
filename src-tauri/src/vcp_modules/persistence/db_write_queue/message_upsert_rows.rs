use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::ToSql;

pub(super) fn insert_message_rows(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
    compressed_contents: &[Vec<u8>],
    render_bytes: &[Vec<u8>],
) -> rusqlite::Result<()> {
    const MAX_PARAMS: usize = 999;
    const PARAMS_PER_MESSAGE: usize = 15;
    let indices = (0..messages.len()).collect::<Vec<_>>();
    for chunk in indices.chunks(MAX_PARAMS / PARAMS_PER_MESSAGE) {
        insert_message_chunk(tx, key, messages, compressed_contents, render_bytes, chunk)?;
    }
    Ok(())
}

fn insert_message_chunk(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
    compressed_contents: &[Vec<u8>],
    render_bytes: &[Vec<u8>],
    indices: &[usize],
) -> rusqlite::Result<()> {
    let values = vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"; indices.len()].join(", ");
    let sql = format!(
        "INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id, content, timestamp,
            is_group_message, group_id, finish_reason, content_hash, created_at, updated_at
        ) VALUES {values}
        ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
            content = excluded.content, role = excluded.role, name = excluded.name,
            agent_id = excluded.agent_id, timestamp = excluded.timestamp,
            is_group_message = excluded.is_group_message, group_id = excluded.group_id,
            finish_reason = excluded.finish_reason, content_hash = excluded.content_hash,
            updated_at = excluded.updated_at
        WHERE messages.deleted_at IS NULL"
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(indices.len() * 15);
    for index in indices {
        let message = &messages[*index];
        let content_hash = HashAggregator::compute_message_fingerprint_for_dto(message);
        let timestamp = checked_i64(message.timestamp, "message timestamp")?;
        let updated_at = checked_i64(message.updated_at, "message updatedAt")?;
        params.extend([
            Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
            Box::new(key.owner_id.clone()),
            Box::new(key.topic_id.clone()),
            Box::new(message.id.clone()),
            Box::new(message.role.clone()),
            Box::new(message.name.clone()),
            Box::new(message.agent_id.clone()),
            Box::new(compressed_contents[*index].clone()),
            Box::new(timestamp),
            Box::new(message.is_group_message.unwrap_or(false)),
            Box::new(message.group_id.clone()),
            Box::new(message.finish_reason.clone()),
            Box::new(content_hash),
            Box::new(timestamp),
            Box::new(updated_at),
        ]);
    }
    let refs = params
        .iter()
        .map(|param| param.as_ref())
        .collect::<Vec<_>>();
    tx.execute(&sql, &*refs)?;
    write_render_rows(tx, key, messages, render_bytes)
}

fn write_render_rows(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
    render_bytes: &[Vec<u8>],
) -> rusqlite::Result<()> {
    for (message, render) in messages.iter().zip(render_bytes) {
        let updated_at = checked_i64(message.updated_at, "render updatedAt")?;
        if render.is_empty() {
            tx.execute(
                "DELETE FROM render_cache
                 WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
                rusqlite::params![&key.owner_type, &key.owner_id, &key.topic_id, &message.id],
            )?;
            continue;
        }
        let content_hash = HashAggregator::compute_message_fingerprint_for_dto(message);
        tx.execute(
            "INSERT INTO render_cache (
                owner_type, owner_id, topic_id, msg_id, render_content, updated_at,
                content_hash, renderer_schema_version
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 1)
            ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
                render_content = excluded.render_content, updated_at = excluded.updated_at,
                content_hash = excluded.content_hash,
                renderer_schema_version = excluded.renderer_schema_version",
            rusqlite::params![
                &key.owner_type,
                &key.owner_id,
                &key.topic_id,
                &message.id,
                render,
                updated_at,
                content_hash
            ],
        )?;
    }
    Ok(())
}

fn checked_i64(value: u64, label: &str) -> rusqlite::Result<i64> {
    i64::try_from(value).map_err(|_| {
        crate::vcp_modules::db_write_queue::DbWriteQueue::sync_contract_error(format!(
            "{label} exceeds SQLite integer range"
        ))
    })
}
