use crate::vcp_modules::topic_types::TopicKey;
use rusqlite::{ToSql, Transaction};

pub(super) fn delete_message_unread_receipts(
    tx: &Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    if !unread_receipts_exist(tx)? || message_ids.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; message_ids.len()].join(", ");
    let counted_unread = count_counted_unread(tx, key, message_ids, &placeholders)?;
    delete_receipt_rows(tx, key, message_ids, &placeholders)?;
    decrement_topic_unread_count(tx, key, counted_unread)
}

fn unread_receipts_exist(tx: &Transaction<'_>) -> rusqlite::Result<bool> {
    tx.query_row(
        "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'message_unread_receipts'
            )",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists != 0)
}

fn count_counted_unread(
    tx: &Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
    placeholders: &str,
) -> rusqlite::Result<i64> {
    let count_sql = format!(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND counted_unread = 1 AND msg_id IN ({placeholders})"
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(message_ids.len() + 3);
    params.extend([
        Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
        Box::new(key.owner_id.clone()),
        Box::new(key.topic_id.clone()),
    ]);
    params.extend(
        message_ids
            .iter()
            .cloned()
            .map(|id| Box::new(id) as Box<dyn ToSql>),
    );
    let refs = params
        .iter()
        .map(|param| param.as_ref())
        .collect::<Vec<_>>();
    tx.query_row(&count_sql, &*refs, |row| row.get(0))
}

fn delete_receipt_rows(
    tx: &Transaction<'_>,
    key: &TopicKey,
    message_ids: &[String],
    placeholders: &str,
) -> rusqlite::Result<()> {
    let sql = format!(
        "DELETE FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id IN ({placeholders})"
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::with_capacity(message_ids.len() + 3);
    params.extend([
        Box::new(key.owner_type.clone()) as Box<dyn ToSql>,
        Box::new(key.owner_id.clone()),
        Box::new(key.topic_id.clone()),
    ]);
    params.extend(
        message_ids
            .iter()
            .cloned()
            .map(|id| Box::new(id) as Box<dyn ToSql>),
    );
    let refs = params
        .iter()
        .map(|param| param.as_ref())
        .collect::<Vec<_>>();
    tx.execute(&sql, &*refs)?;
    Ok(())
}

fn decrement_topic_unread_count(
    tx: &Transaction<'_>,
    key: &TopicKey,
    counted_unread: i64,
) -> rusqlite::Result<()> {
    if counted_unread <= 0 {
        return Ok(());
    }
    let has_unread_count: bool = tx.query_row(
        "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('topics') WHERE name = 'unread_count'
            )",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if has_unread_count {
        tx.execute(
            "UPDATE topics
             SET unread_count = MAX(unread_count - ?1, 0),
                 unread = CASE WHEN MAX(unread_count - ?1, 0) = 0 THEN 0 ELSE unread END,
                 updated_at = MAX(COALESCE(updated_at, 0), ?2)
             WHERE owner_type = ?3 AND owner_id = ?4 AND topic_id = ?5
               AND deleted_at IS NULL",
            rusqlite::params![
                counted_unread,
                crate::vcp_modules::infra::utils::now_millis(),
                &key.owner_type,
                &key.owner_id,
                &key.topic_id,
            ],
        )?;
    }
    Ok(())
}
