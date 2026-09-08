use super::rebuild_lock;
use super::status::{inspect_schema, SchemaInfo};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::{sqlite::SqliteRow, Connection, Row, Sqlite, SqliteConnection, SqlitePool, Transaction};
use std::time::Instant;

const REBUILD_BATCH_SIZE: i64 = 128;
const REBUILD_MMAP_DISABLED: i64 = 0;

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
    let mut connection = pool
        .acquire()
        .await
        .map_err(|_| "FTS_REBUILD_BEGIN_FAILED".to_string())?;
    let indexed_count = rebuild_on_connection(&mut connection, schema).await?;
    Ok(FtsRebuildResult {
        indexed_count,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

struct RebuildMmapGuard {
    original_size: i64,
}

impl RebuildMmapGuard {
    async fn prepare(connection: &mut SqliteConnection) -> Result<Self, String> {
        let original_size = sqlx::query_scalar::<_, i64>("PRAGMA mmap_size")
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| "FTS_REBUILD_BEGIN_FAILED".to_string())?
            .unwrap_or(REBUILD_MMAP_DISABLED);
        let guard = Self { original_size };
        if guard.disable(connection).await.is_err() {
            let _ = guard.restore(connection).await;
            return Err("FTS_REBUILD_BEGIN_FAILED".to_string());
        }
        Ok(guard)
    }

    async fn disable(&self, connection: &mut SqliteConnection) -> Result<(), ()> {
        sqlx::query("PRAGMA mmap_size = 0")
            .execute(&mut *connection)
            .await
            .map_err(|_| ())?;
        let mmap_size = sqlx::query_scalar::<_, i64>("PRAGMA mmap_size")
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| ())?;
        if mmap_size.is_some_and(|size| size != REBUILD_MMAP_DISABLED) {
            return Err(());
        }
        sqlx::query("PRAGMA shrink_memory")
            .execute(&mut *connection)
            .await
            .map_err(|_| ())?;
        Ok(())
    }

    async fn restore(&self, connection: &mut SqliteConnection) -> Result<(), String> {
        let mut failed = false;
        if sqlx::query("PRAGMA shrink_memory")
            .execute(&mut *connection)
            .await
            .is_err()
        {
            failed = true;
        }

        let restore_sql = format!("PRAGMA mmap_size = {}", self.original_size);
        if sqlx::query(&restore_sql)
            .execute(&mut *connection)
            .await
            .is_err()
        {
            failed = true;
        }
        match sqlx::query_scalar::<_, i64>("PRAGMA mmap_size")
            .fetch_optional(&mut *connection)
            .await
        {
            Ok(Some(mmap_size)) if mmap_size == self.original_size => {}
            Ok(None) if self.original_size == REBUILD_MMAP_DISABLED => {}
            _ => failed = true,
        }

        if failed {
            Err("FTS_REBUILD_MMAP_RESTORE_FAILED".to_string())
        } else {
            Ok(())
        }
    }
}

async fn rebuild_on_connection(
    connection: &mut sqlx::pool::PoolConnection<Sqlite>,
    schema: SchemaInfo,
) -> Result<i64, String> {
    let mmap_guard = match RebuildMmapGuard::prepare(&mut **connection).await {
        Ok(guard) => guard,
        Err(error) => {
            connection.close_on_drop();
            return Err(error);
        }
    };
    let result = rebuild_transaction(&mut **connection, schema).await;
    let restore_result = mmap_guard.restore(&mut **connection).await;
    if restore_result.is_err() {
        connection.close_on_drop();
    }
    match (result, restore_result) {
        (Ok(indexed_count), Ok(())) => Ok(indexed_count),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(restore_error)) => Err(restore_error),
        (Err(error), Err(restore_error)) => Err(format!("{error}; {restore_error}")),
    }
}

async fn rebuild_transaction(
    connection: &mut SqliteConnection,
    schema: SchemaInfo,
) -> Result<i64, String> {
    let mut transaction = connection
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "FTS_REBUILD_BEGIN_FAILED".to_string())?;
    let result = rebuild_in_transaction(&mut transaction, schema).await;
    match result {
        Ok(indexed_count) => transaction
            .commit()
            .await
            .map(|_| indexed_count)
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

#[cfg(test)]
mod tests {
    use super::RebuildMmapGuard;
    use sqlx::{sqlite::SqliteConnectOptions, Connection, SqliteConnection};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_database_path() -> std::path::PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "vcp-global-search-rebuild-mmap-{}-{id}.db",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn rebuild_mmap_setting_is_restored_after_temporary_disable() {
        let path = test_database_path();
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .pragma("mmap_size", "65536");
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .expect("应打开 mmap 设置测试数据库");
        sqlx::query("CREATE TABLE messages (content TEXT NOT NULL)")
            .execute(&mut connection)
            .await
            .expect("应创建 mmap 设置测试表");
        sqlx::query("PRAGMA mmap_size = 65536")
            .execute(&mut connection)
            .await
            .expect("应设置测试 mmap 大小");
        let original_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
            .fetch_one(&mut connection)
            .await
            .expect("应读取原始 mmap 大小");
        assert!(original_size > 0, "测试数据库必须支持非零 mmap 大小");

        let guard = RebuildMmapGuard::prepare(&mut connection)
            .await
            .expect("应临时禁用 mmap");
        let disabled_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
            .fetch_one(&mut connection)
            .await
            .expect("应读取禁用后的 mmap 大小");
        assert_eq!(disabled_size, 0);

        guard
            .restore(&mut connection)
            .await
            .expect("应恢复 mmap 设置");
        let restored_size: i64 = sqlx::query_scalar("PRAGMA mmap_size")
            .fetch_one(&mut connection)
            .await
            .expect("应读取恢复后的 mmap 大小");
        assert_eq!(restored_size, original_size);

        connection
            .close()
            .await
            .expect("应关闭 mmap 设置测试数据库");
        let _ = std::fs::remove_file(path);
    }
}
