use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use std::collections::{HashMap, HashSet};

use sqlx::{sqlite::SqliteRow, Row, SqliteConnection};

pub(super) const STATUS_BATCH_SIZE: i64 = 256;
pub(super) const STALE_FIRST_BATCH_SQL: &str =
    "SELECT m.rowid AS message_rowid, f.rowid AS fts_rowid,
            m.content AS stored_content, f.content AS indexed_content
     FROM messages_fts f
     INNER JOIN messages m
       ON f.owner_type = m.owner_type AND f.owner_id = m.owner_id
      AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
     WHERE m.deleted_at IS NULL
     ORDER BY f.rowid
     LIMIT ?";
pub(super) const STALE_BATCH_SQL: &str = "SELECT m.rowid AS message_rowid, f.rowid AS fts_rowid,
            m.content AS stored_content, f.content AS indexed_content
     FROM messages_fts f
     INNER JOIN messages m
       ON f.owner_type = m.owner_type AND f.owner_id = m.owner_id
      AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
     WHERE m.deleted_at IS NULL
       AND f.rowid > ?
     ORDER BY f.rowid
     LIMIT ?";
pub(super) const LIVE_IDENTITY_FIRST_BATCH_SQL: &str =
    "SELECT rowid AS message_rowid, owner_type, owner_id, topic_id, msg_id
     FROM messages
     WHERE deleted_at IS NULL
     ORDER BY rowid
     LIMIT ?";
pub(super) const LIVE_IDENTITY_BATCH_SQL: &str =
    "SELECT rowid AS message_rowid, owner_type, owner_id, topic_id, msg_id
     FROM messages
     WHERE deleted_at IS NULL
       AND rowid > ?
     ORDER BY rowid
     LIMIT ?";
pub(super) const FTS_IDENTITY_FIRST_BATCH_SQL: &str =
    "SELECT rowid AS fts_rowid, owner_type, owner_id, topic_id, msg_id
     FROM messages_fts
     ORDER BY rowid
     LIMIT ?";
pub(super) const FTS_IDENTITY_BATCH_SQL: &str =
    "SELECT rowid AS fts_rowid, owner_type, owner_id, topic_id, msg_id
     FROM messages_fts
     WHERE rowid > ?
     ORDER BY rowid
     LIMIT ?";

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CountSnapshot {
    pub(super) topic_count: i64,
    pub(super) live_topic_row_count: i64,
    pub(super) live_count: i64,
    pub(super) indexed_count: i64,
    pub(super) missing_count: i64,
    pub(super) orphan_count: i64,
    pub(super) duplicate_count: i64,
}

#[derive(Debug, Eq, Hash, PartialEq)]
struct MessageIdentity {
    owner_type: String,
    owner_id: String,
    topic_id: String,
    msg_id: String,
}

#[derive(Debug, Default)]
pub(super) struct StaleSnapshot {
    pub(super) stale_count: i64,
    pub(super) decode_error_count: i64,
    pub(super) decoded_content_bytes: i64,
    decode_error_rowids: HashSet<i64>,
    decoded_rowids: HashSet<i64>,
}

/// Touch both source tables so a caller's deferred transaction owns one read snapshot.
#[cfg(test)]
pub(super) async fn establish_snapshot(connection: &mut SqliteConnection) -> Result<(), String> {
    sqlx::query("SELECT (SELECT COUNT(*) FROM messages) + (SELECT COUNT(*) FROM messages_fts)")
        .fetch_one(&mut *connection)
        .await
        .map(|_| ())
        .map_err(|_| "FTS_STATUS_SNAPSHOT_FAILED".to_string())
}

pub(super) async fn load_counts(
    connection: &mut SqliteConnection,
) -> Result<CountSnapshot, String> {
    let topic_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
         FROM (
             SELECT DISTINCT owner_type, owner_id, topic_id
             FROM messages
             WHERE deleted_at IS NULL
         )",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?;
    let live_topic_row_count = load_live_topic_row_count(connection).await?;
    let live_identities = load_live_identities(connection).await?;
    let live_count = live_identities.len() as i64;
    let (indexed_count, orphan_count, duplicate_count, missing_count) =
        scan_index_identities(connection, live_identities).await?;
    Ok(CountSnapshot {
        topic_count,
        live_topic_row_count,
        live_count,
        indexed_count,
        missing_count,
        orphan_count,
        duplicate_count,
    })
}

async fn load_live_topic_row_count(connection: &mut SqliteConnection) -> Result<i64, String> {
    let table_exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'topics'",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?;
    if table_exists == 0 {
        return Ok(0);
    }
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics WHERE deleted_at IS NULL")
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())
}

async fn load_live_identities(
    connection: &mut SqliteConnection,
) -> Result<HashMap<MessageIdentity, bool>, String> {
    let mut live_identities = HashMap::new();
    let mut cursor = None;
    loop {
        let rows = load_identity_page(connection, false, cursor).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some(
            last.try_get("message_rowid")
                .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
        );
        for row in rows {
            live_identities.insert(message_identity(&row)?, false);
        }
    }
    Ok(live_identities)
}

async fn scan_index_identities(
    connection: &mut SqliteConnection,
    mut live_identities: HashMap<MessageIdentity, bool>,
) -> Result<(i64, i64, i64, i64), String> {
    let mut cursor = None;
    let mut indexed_count = 0_i64;
    let mut orphan_count = 0_i64;
    let mut duplicate_count = 0_i64;
    let mut seen_identities = HashSet::new();
    loop {
        let rows = load_identity_page(connection, true, cursor).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some(
            last.try_get("fts_rowid")
                .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
        );
        for row in rows {
            indexed_count += 1;
            let identity = message_identity(&row)?;
            if let Some(matched) = live_identities.get_mut(&identity) {
                *matched = true;
            } else {
                orphan_count += 1;
            }
            if !seen_identities.insert(identity) {
                duplicate_count += 1;
            }
        }
    }
    let missing_count = live_identities
        .values()
        .filter(|matched| !**matched)
        .count() as i64;
    Ok((indexed_count, orphan_count, duplicate_count, missing_count))
}

fn message_identity(row: &SqliteRow) -> Result<MessageIdentity, String> {
    Ok(MessageIdentity {
        owner_type: row
            .try_get("owner_type")
            .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
        owner_id: row
            .try_get("owner_id")
            .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
        topic_id: row
            .try_get("topic_id")
            .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
        msg_id: row
            .try_get("msg_id")
            .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())?,
    })
}

async fn load_rowid_batch(
    connection: &mut SqliteConnection,
    first_sql: &str,
    next_sql: &str,
    cursor: Option<i64>,
    error_code: &str,
) -> Result<Vec<SqliteRow>, String> {
    let result = if let Some(cursor) = cursor {
        sqlx::query(next_sql)
            .bind(cursor)
            .bind(STATUS_BATCH_SIZE)
            .fetch_all(&mut *connection)
            .await
    } else {
        sqlx::query(first_sql)
            .bind(STATUS_BATCH_SIZE)
            .fetch_all(&mut *connection)
            .await
    };
    result.map_err(|_| error_code.to_string())
}

pub(super) async fn load_identity_page(
    connection: &mut SqliteConnection,
    fts: bool,
    cursor: Option<i64>,
) -> Result<Vec<SqliteRow>, String> {
    if fts {
        load_rowid_batch(
            connection,
            FTS_IDENTITY_FIRST_BATCH_SQL,
            FTS_IDENTITY_BATCH_SQL,
            cursor,
            "FTS_STATUS_COUNT_FAILED",
        )
        .await
    } else {
        load_rowid_batch(
            connection,
            LIVE_IDENTITY_FIRST_BATCH_SQL,
            LIVE_IDENTITY_BATCH_SQL,
            cursor,
            "FTS_STATUS_COUNT_FAILED",
        )
        .await
    }
}

pub(super) async fn count_stale_rows(
    connection: &mut SqliteConnection,
) -> Result<StaleSnapshot, String> {
    let mut snapshot = StaleSnapshot::default();
    let mut cursor = None;
    let mut decoded = None;
    loop {
        let rows = load_stale_batch(connection, cursor).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = Some(
            last.try_get("fts_rowid")
                .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?,
        );
        inspect_stale_batch(&rows, &mut snapshot, &mut decoded)?;
    }
    Ok(snapshot)
}

async fn load_stale_batch(
    connection: &mut SqliteConnection,
    cursor: Option<i64>,
) -> Result<Vec<SqliteRow>, String> {
    load_rowid_batch(
        connection,
        STALE_FIRST_BATCH_SQL,
        STALE_BATCH_SQL,
        cursor,
        "FTS_STATUS_STALE_SCAN_FAILED",
    )
    .await
}

fn inspect_stale_batch(
    rows: &[SqliteRow],
    snapshot: &mut StaleSnapshot,
    decoded: &mut Option<(i64, Result<String, String>)>,
) -> Result<(), String> {
    for row in rows {
        let message_rowid: i64 = row
            .try_get("message_rowid")
            .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?;
        if decoded
            .as_ref()
            .is_none_or(|(rowid, _)| *rowid != message_rowid)
        {
            *decoded = Some((message_rowid, decode_message_content(row, "stored_content")));
        }
        let Some((_, result)) = decoded.as_ref() else {
            continue;
        };
        match result {
            Ok(content) => {
                if snapshot.decoded_rowids.insert(message_rowid) {
                    let content_bytes = i64::try_from(content.len())
                        .map_err(|_| "FTS_STATUS_DECODED_BYTES_OVERFLOW".to_string())?;
                    snapshot.decoded_content_bytes = snapshot
                        .decoded_content_bytes
                        .checked_add(content_bytes)
                        .ok_or_else(|| "FTS_STATUS_DECODED_BYTES_OVERFLOW".to_string())?;
                }
                let indexed: String = row
                    .try_get("indexed_content")
                    .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?;
                if content != &indexed {
                    snapshot.stale_count += 1;
                }
            }
            Err(_) if snapshot.decode_error_rowids.insert(message_rowid) => {
                snapshot.decode_error_count += 1;
                log::warn!("[FTS] 完整性扫描解码消息正文失败（rowid={message_rowid}）");
            }
            Err(_) => {}
        }
    }
    Ok(())
}
