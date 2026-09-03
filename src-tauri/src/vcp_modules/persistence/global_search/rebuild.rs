use super::rebuild_lock;
use super::status::{inspect_schema, SchemaInfo};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::{sqlite::SqliteRow, Row, Sqlite, SqlitePool, Transaction};
use std::time::Instant;

const REBUILD_BATCH_SIZE: i64 = 128;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsRebuildResult {
    pub indexed_count: i64,
    pub duration_ms: u64,
}

pub(crate) async fn rebuild_messages_fts(pool: &SqlitePool) -> Result<FtsRebuildResult, String> {
    let _guard = rebuild_lock().lock().await;
    let schema = inspect_schema(pool).await?;
    let started = Instant::now();
    let mut transaction = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "FTS_REBUILD_BEGIN_FAILED".to_string())?;
    let result = rebuild_in_transaction(&mut transaction, schema).await;
    match result {
        Ok(indexed_count) => transaction
            .commit()
            .await
            .map(|_| FtsRebuildResult {
                indexed_count,
                duration_ms: started.elapsed().as_millis() as u64,
            })
            .map_err(|_| "FTS_REBUILD_COMMIT_FAILED".to_string()),
        Err(error) => match transaction.rollback().await {
            Ok(()) => Err(error),
            Err(_) => Err(format!("{error}; FTS_REBUILD_ROLLBACK_FAILED")),
        },
    }
}

async fn rebuild_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    schema: SchemaInfo,
) -> Result<i64, String> {
    ensure_rebuild_schema(transaction, schema).await?;
    sqlx::query("DELETE FROM messages_fts")
        .execute(&mut **transaction)
        .await
        .map_err(|_| "FTS_REBUILD_CLEAR_FAILED".to_string())?;
    let mut last_rowid = 0_i64;
    let mut indexed_count = 0_i64;
    loop {
        let rows = load_message_batch(transaction, last_rowid).await?;
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            insert_index_row(transaction, row).await?;
        }
        indexed_count += rows.len() as i64;
        last_rowid = rowid_of(rows.last().expect("批次不能为空"))?;
    }
    Ok(indexed_count)
}

async fn ensure_rebuild_schema(
    transaction: &mut Transaction<'_, Sqlite>,
    schema: SchemaInfo,
) -> Result<(), String> {
    if schema.available() {
        return Ok(());
    }
    sqlx::query("DROP TABLE IF EXISTS messages_fts")
        .execute(&mut **transaction)
        .await
        .map_err(|_| "FTS_REBUILD_SCHEMA_DROP_FAILED".to_string())?;
    sqlx::query(
        "CREATE VIRTUAL TABLE messages_fts USING fts5(
            msg_id UNINDEXED,
            topic_id UNINDEXED,
            content,
            owner_type UNINDEXED,
            owner_id UNINDEXED,
            tokenize = 'trigram'
        )",
    )
    .execute(&mut **transaction)
    .await
    .map(|_| ())
    .map_err(|_| "FTS_REBUILD_SCHEMA_CREATE_FAILED".to_string())
}

async fn load_message_batch(
    transaction: &mut Transaction<'_, Sqlite>,
    last_rowid: i64,
) -> Result<Vec<SqliteRow>, String> {
    sqlx::query(
        "SELECT rowid, owner_type, owner_id, topic_id, msg_id, content
         FROM messages
         WHERE deleted_at IS NULL AND rowid > ?
         ORDER BY rowid
         LIMIT ?",
    )
    .bind(last_rowid)
    .bind(REBUILD_BATCH_SIZE)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())
}

async fn insert_index_row(
    transaction: &mut Transaction<'_, Sqlite>,
    row: &SqliteRow,
) -> Result<(), String> {
    let rowid = rowid_of(row)?;
    let content = decode_message_content(row, "content")
        .map_err(|_| format!("FTS_REBUILD_CONTENT_DECODE_FAILED(rowid={rowid})"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())?;
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())?;
    let msg_id: String = row
        .try_get("msg_id")
        .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())?;
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(topic_id)
    .bind(content)
    .bind(owner_type)
    .bind(owner_id)
    .execute(&mut **transaction)
    .await
    .map(|_| ())
    .map_err(|_| "FTS_REBUILD_INSERT_FAILED".to_string())
}

fn rowid_of(row: &SqliteRow) -> Result<i64, String> {
    row.try_get("rowid")
        .map_err(|_| "FTS_REBUILD_READ_FAILED".to_string())
}
