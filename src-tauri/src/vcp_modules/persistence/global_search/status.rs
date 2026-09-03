use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};

const STATUS_BATCH_SIZE: i64 = 256;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsIndexStatus {
    pub available: bool,
    pub schema_valid: bool,
    pub tokenizer_valid: bool,
    pub live_count: i64,
    pub indexed_count: i64,
    pub missing_count: i64,
    pub orphan_count: i64,
    pub duplicate_count: i64,
    pub stale_count: i64,
    pub decode_error_count: i64,
    pub healthy: bool,
    pub diagnostic: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SchemaInfo {
    pub(crate) schema_valid: bool,
    pub(crate) tokenizer_valid: bool,
}

impl SchemaInfo {
    pub(crate) fn available(self) -> bool {
        self.schema_valid && self.tokenizer_valid
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CountSnapshot {
    live_count: i64,
    indexed_count: i64,
    missing_count: i64,
    orphan_count: i64,
    duplicate_count: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct StaleSnapshot {
    stale_count: i64,
    decode_error_count: i64,
    last_decode_error_rowid: Option<i64>,
}

pub(crate) async fn get_fts_index_status(pool: &SqlitePool) -> Result<FtsIndexStatus, String> {
    let schema = inspect_schema(pool).await?;
    if !schema.available() {
        return Ok(unavailable_status(schema));
    }

    let counts = load_counts(pool).await?;
    let stale = count_stale_rows(pool).await?;
    let healthy = is_healthy(counts, stale);
    Ok(FtsIndexStatus {
        available: true,
        schema_valid: schema.schema_valid,
        tokenizer_valid: schema.tokenizer_valid,
        live_count: counts.live_count,
        indexed_count: counts.indexed_count,
        missing_count: counts.missing_count,
        orphan_count: counts.orphan_count,
        duplicate_count: counts.duplicate_count,
        stale_count: stale.stale_count,
        decode_error_count: stale.decode_error_count,
        healthy,
        diagnostic: diagnostic_for(stale.decode_error_count),
    })
}

pub(crate) async fn inspect_schema(pool: &SqlitePool) -> Result<SchemaInfo, String> {
    let create_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
    )
    .fetch_optional(pool)
    .await
    .map_err(|_| "FTS_SCHEMA_INSPECTION_FAILED".to_string())?;
    let Some(create_sql) = create_sql else {
        return Ok(SchemaInfo::default());
    };

    let columns = sqlx::query("PRAGMA table_info(messages_fts)")
        .fetch_all(pool)
        .await
        .map_err(|_| "FTS_SCHEMA_INSPECTION_FAILED".to_string())?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<std::collections::HashSet<_>>();
    let required = ["msg_id", "topic_id", "content", "owner_type", "owner_id"];
    let schema_valid = required.iter().all(|name| columns.contains(*name));
    let compact_sql = create_sql
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    let tokenizer_valid =
        compact_sql.contains("tokenize='trigram'") || compact_sql.contains("tokenize=\"trigram\"");
    Ok(SchemaInfo {
        schema_valid,
        tokenizer_valid,
    })
}

fn unavailable_status(schema: SchemaInfo) -> FtsIndexStatus {
    FtsIndexStatus {
        available: false,
        schema_valid: schema.schema_valid,
        tokenizer_valid: schema.tokenizer_valid,
        live_count: 0,
        indexed_count: 0,
        missing_count: 0,
        orphan_count: 0,
        duplicate_count: 0,
        stale_count: 0,
        decode_error_count: 0,
        healthy: false,
        diagnostic: Some("FTS_SCHEMA_UNAVAILABLE".to_string()),
    }
}

async fn load_counts(pool: &SqlitePool) -> Result<CountSnapshot, String> {
    Ok(CountSnapshot {
        live_count: scalar_count(
            pool,
            "SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL",
        )
        .await?,
        indexed_count: scalar_count(pool, "SELECT COUNT(*) FROM messages_fts").await?,
        missing_count: scalar_count(
            pool,
            "SELECT COUNT(*)
             FROM messages m
             LEFT JOIN messages_fts f
               ON f.owner_type = m.owner_type AND f.owner_id = m.owner_id
              AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
             WHERE m.deleted_at IS NULL AND f.rowid IS NULL",
        )
        .await?,
        orphan_count: scalar_count(
            pool,
            "SELECT COUNT(*)
             FROM messages_fts f
             LEFT JOIN messages m
               ON m.owner_type = f.owner_type AND m.owner_id = f.owner_id
              AND m.topic_id = f.topic_id AND m.msg_id = f.msg_id
              AND m.deleted_at IS NULL
             WHERE m.rowid IS NULL",
        )
        .await?,
        duplicate_count: scalar_count(
            pool,
            "SELECT COALESCE(SUM(duplicate_rows), 0)
             FROM (
                 SELECT COUNT(*) - 1 AS duplicate_rows
                 FROM messages_fts
                 GROUP BY owner_type, owner_id, topic_id, msg_id
                 HAVING COUNT(*) > 1
             )",
        )
        .await?,
    })
}

async fn scalar_count(pool: &SqlitePool, sql: &str) -> Result<i64, String> {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .map_err(|_| "FTS_STATUS_COUNT_FAILED".to_string())
}

async fn count_stale_rows(pool: &SqlitePool) -> Result<StaleSnapshot, String> {
    let mut snapshot = StaleSnapshot::default();
    let mut cursor = (0_i64, -1_i64);
    loop {
        let rows = load_stale_batch(pool, cursor).await?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = (
            last.try_get("message_rowid")
                .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?,
            last.try_get("fts_rowid")
                .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?,
        );
        inspect_stale_batch(&rows, &mut snapshot)?;
    }
    Ok(snapshot)
}

async fn load_stale_batch(pool: &SqlitePool, cursor: (i64, i64)) -> Result<Vec<SqliteRow>, String> {
    sqlx::query(
        "SELECT m.rowid AS message_rowid, f.rowid AS fts_rowid,
                m.content AS stored_content, f.content AS indexed_content
         FROM messages m
         INNER JOIN messages_fts f
           ON f.owner_type = m.owner_type AND f.owner_id = m.owner_id
          AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
         WHERE m.deleted_at IS NULL
           AND (m.rowid > ? OR (m.rowid = ? AND f.rowid > ?))
         ORDER BY m.rowid, f.rowid
         LIMIT ?",
    )
    .bind(cursor.0)
    .bind(cursor.0)
    .bind(cursor.1)
    .bind(STATUS_BATCH_SIZE)
    .fetch_all(pool)
    .await
    .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())
}

fn inspect_stale_batch(rows: &[SqliteRow], snapshot: &mut StaleSnapshot) -> Result<(), String> {
    let mut decoded = None;
    for row in rows {
        let message_rowid: i64 = row
            .try_get("message_rowid")
            .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?;
        if decoded
            .as_ref()
            .is_none_or(|(rowid, _)| *rowid != message_rowid)
        {
            decoded = Some((message_rowid, decode_message_content(row, "stored_content")));
        }
        let Some((_, result)) = decoded.as_ref() else {
            continue;
        };
        match result {
            Ok(content) => {
                let indexed: String = row
                    .try_get("indexed_content")
                    .map_err(|_| "FTS_STATUS_STALE_SCAN_FAILED".to_string())?;
                if content != &indexed {
                    snapshot.stale_count += 1;
                }
            }
            Err(_) if snapshot.last_decode_error_rowid != Some(message_rowid) => {
                snapshot.decode_error_count += 1;
                snapshot.last_decode_error_rowid = Some(message_rowid);
                log::warn!("[FTS] 完整性扫描解码消息正文失败（rowid={message_rowid}）");
            }
            Err(_) => {}
        }
    }
    Ok(())
}

fn is_healthy(counts: CountSnapshot, stale: StaleSnapshot) -> bool {
    counts.missing_count == 0
        && counts.orphan_count == 0
        && counts.duplicate_count == 0
        && stale.stale_count == 0
        && stale.decode_error_count == 0
        && counts.live_count == counts.indexed_count
}

fn diagnostic_for(decode_error_count: i64) -> Option<String> {
    (decode_error_count > 0).then(|| "FTS_CONTENT_DECODE_FAILED".to_string())
}
