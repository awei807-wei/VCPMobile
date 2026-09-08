use std::collections::HashSet;

use sqlx::{Row, SqliteConnection, SqlitePool};

use super::status_scan::{self, CountSnapshot, StaleSnapshot};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsIndexStatus {
    pub available: bool,
    pub schema_valid: bool,
    pub tokenizer_valid: bool,
    pub topic_count: i64,
    pub live_topic_row_count: i64,
    pub live_count: i64,
    pub indexed_count: i64,
    pub missing_count: i64,
    pub orphan_count: i64,
    pub duplicate_count: i64,
    pub stale_count: i64,
    pub decode_error_count: i64,
    pub decoded_content_bytes: i64,
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

pub(crate) async fn get_fts_index_status(pool: &SqlitePool) -> Result<FtsIndexStatus, String> {
    // BEGIN is SQLite's deferred mode; all status stages below use this one connection.
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| "FTS_STATUS_TRANSACTION_FAILED".to_string())?;
    let result = status_in_connection(&mut *transaction).await;
    match result {
        Ok(status) => {
            transaction
                .commit()
                .await
                .map_err(|_| "FTS_STATUS_TRANSACTION_FAILED".to_string())?;
            Ok(status)
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

pub(super) async fn status_in_connection(
    connection: &mut SqliteConnection,
) -> Result<FtsIndexStatus, String> {
    let schema = inspect_schema_on_connection(connection).await?;
    if !schema.available() {
        return Ok(unavailable_status(schema));
    }

    let counts = status_scan::load_counts(connection).await?;
    let stale = status_scan::count_stale_rows(connection).await?;
    Ok(aggregate_status(schema, counts, stale))
}

fn aggregate_status(
    schema: SchemaInfo,
    counts: CountSnapshot,
    stale: StaleSnapshot,
) -> FtsIndexStatus {
    let healthy = is_healthy(counts, &stale);
    FtsIndexStatus {
        available: true,
        schema_valid: schema.schema_valid,
        tokenizer_valid: schema.tokenizer_valid,
        topic_count: counts.topic_count,
        live_topic_row_count: counts.live_topic_row_count,
        live_count: counts.live_count,
        indexed_count: counts.indexed_count,
        missing_count: counts.missing_count,
        orphan_count: counts.orphan_count,
        duplicate_count: counts.duplicate_count,
        stale_count: stale.stale_count,
        decode_error_count: stale.decode_error_count,
        decoded_content_bytes: stale.decoded_content_bytes,
        healthy,
        diagnostic: diagnostic_for(stale.decode_error_count),
    }
}

pub(crate) async fn inspect_schema(pool: &SqlitePool) -> Result<SchemaInfo, String> {
    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| "FTS_SCHEMA_INSPECTION_FAILED".to_string())?;
    inspect_schema_on_connection(&mut *connection).await
}

async fn inspect_schema_on_connection(
    connection: &mut SqliteConnection,
) -> Result<SchemaInfo, String> {
    let create_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| "FTS_SCHEMA_INSPECTION_FAILED".to_string())?;
    let Some(create_sql) = create_sql else {
        return Ok(SchemaInfo::default());
    };

    let columns = sqlx::query("PRAGMA table_info(messages_fts)")
        .fetch_all(&mut *connection)
        .await
        .map_err(|_| "FTS_SCHEMA_INSPECTION_FAILED".to_string())?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<HashSet<_>>();
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
        topic_count: 0,
        live_topic_row_count: 0,
        live_count: 0,
        indexed_count: 0,
        missing_count: 0,
        orphan_count: 0,
        duplicate_count: 0,
        stale_count: 0,
        decode_error_count: 0,
        decoded_content_bytes: 0,
        healthy: false,
        diagnostic: Some("FTS_SCHEMA_UNAVAILABLE".to_string()),
    }
}

fn is_healthy(counts: CountSnapshot, stale: &StaleSnapshot) -> bool {
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
