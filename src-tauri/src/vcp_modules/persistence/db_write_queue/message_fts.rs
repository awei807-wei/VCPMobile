use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::ToSql;

pub(super) fn refresh_fts(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
) -> rusqlite::Result<()> {
    let ids = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    delete_fts_rows(tx, key, &ids)?;
    insert_fts_rows(tx, key, messages)
}

fn delete_fts_rows(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    ids: &[String],
) -> rusqlite::Result<()> {
    for chunk in ids.chunks(998) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "DELETE FROM messages_fts
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
    Ok(())
}

fn insert_fts_rows(
    tx: &rusqlite::Transaction<'_>,
    key: &TopicKey,
    messages: &[MessageSyncDTO],
) -> rusqlite::Result<()> {
    for chunk in messages.chunks(199) {
        let values = vec!["(?, ?, ?, ?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO messages_fts (msg_id, topic_id, content, owner_type, owner_id)
             VALUES {values}"
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() * 5);
        for message in chunk {
            params.extend([
                Box::new(message.id.clone()) as Box<dyn ToSql>,
                Box::new(key.topic_id.clone()),
                Box::new(message.content.clone()),
                Box::new(key.owner_type.clone()),
                Box::new(key.owner_id.clone()),
            ]);
        }
        let refs = params
            .iter()
            .map(|param| param.as_ref())
            .collect::<Vec<_>>();
        tx.execute(&sql, &*refs)?;
    }
    Ok(())
}
