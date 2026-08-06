use sqlx::{sqlite::SqliteRow, Row, SqlitePool};

use super::message_repository::ContentCompressor;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
const NORMALIZE_BATCH_SIZE: i64 = 500;

/// 从 SQLite 动态类型列中读取消息正文，兼容当前压缩 BLOB 与历史 TEXT/明文 BLOB。
pub fn decode_message_content(row: &SqliteRow, column: &str) -> Result<String, String> {
    match row.try_get::<Option<Vec<u8>>, _>(column) {
        Ok(Some(bytes)) => decode_message_content_bytes(&bytes),
        Ok(None) => Ok(String::new()),
        Err(blob_error) => match row.try_get::<Option<String>, _>(column) {
            Ok(Some(content)) => Ok(content),
            Ok(None) => Ok(String::new()),
            Err(text_error) => Err(format!(
                "消息正文列 {column} 既无法按 BLOB 读取，也无法按 TEXT 读取: BLOB={blob_error}; TEXT={text_error}"
            )),
        },
    }
}

fn decode_message_content_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&ZSTD_MAGIC) {
        return ContentCompressor::decompress(bytes);
    }

    String::from_utf8(bytes.to_vec())
        .map_err(|e| format!("消息正文既不是有效 zstd 数据，也不是 UTF-8 明文 BLOB: {e}"))
}

/// 将历史 TEXT 正文原位转成当前约定的 zstd BLOB，避免后续固定 BLOB 读取发生类型崩溃。
pub async fn normalize_legacy_message_content(pool: &SqlitePool) -> Result<u64, String> {
    let mut last_rowid = 0_i64;
    let mut normalized = 0_u64;

    loop {
        let rows = sqlx::query(
            "SELECT rowid, content
             FROM messages
             WHERE typeof(content) = 'text' AND rowid > ?
             ORDER BY rowid
             LIMIT ?",
        )
        .bind(last_rowid)
        .bind(NORMALIZE_BATCH_SIZE)
        .fetch_all(pool)
        .await
        .map_err(|e| format!("查询历史 TEXT 消息正文失败: {e}"))?;

        if rows.is_empty() {
            break;
        }

        let mut tx = pool
            .begin()
            .await
            .map_err(|e| format!("开始消息正文格式修复事务失败: {e}"))?;

        for row in rows {
            let rowid: i64 = row
                .try_get("rowid")
                .map_err(|e| format!("读取消息 rowid 失败: {e}"))?;
            let content: String = row
                .try_get("content")
                .map_err(|e| format!("读取历史 TEXT 消息正文失败: {e}"))?;
            let compressed = ContentCompressor::compress(&content)?;

            let result = sqlx::query(
                "UPDATE messages
                 SET content = ?
                 WHERE rowid = ? AND typeof(content) = 'text'",
            )
            .bind(compressed)
            .bind(rowid)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("写回压缩消息正文失败: {e}"))?;

            normalized += result.rows_affected();
            last_rowid = rowid;
        }

        tx.commit()
            .await
            .map_err(|e| format!("提交消息正文格式修复事务失败: {e}"))?;
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite should open");
        sqlx::query("CREATE TABLE messages (content TEXT NOT NULL)")
            .execute(&pool)
            .await
            .expect("messages table should be created");
        pool
    }

    #[tokio::test]
    async fn decode_accepts_compressed_blob_text_and_plain_blob() {
        let pool = test_pool().await;
        let compressed = ContentCompressor::compress("压缩正文").expect("content should compress");

        sqlx::query("INSERT INTO messages (content) VALUES (?), (?), (?)")
            .bind(compressed)
            .bind("历史正文")
            .bind(Vec::from("明文 BLOB".as_bytes()))
            .execute(&pool)
            .await
            .expect("fixtures should insert");

        let rows = sqlx::query("SELECT content FROM messages ORDER BY rowid")
            .fetch_all(&pool)
            .await
            .expect("fixtures should load");
        let decoded = rows
            .iter()
            .map(|row| decode_message_content(row, "content").expect("content should decode"))
            .collect::<Vec<_>>();

        assert_eq!(decoded, ["压缩正文", "历史正文", "明文 BLOB"]);
    }

    #[tokio::test]
    async fn normalization_is_lossless_and_idempotent() {
        let pool = test_pool().await;
        let compressed = ContentCompressor::compress("已压缩").expect("content should compress");

        sqlx::query("INSERT INTO messages (content) VALUES (?), (?)")
            .bind("待迁移")
            .bind(compressed)
            .execute(&pool)
            .await
            .expect("fixtures should insert");

        assert_eq!(normalize_legacy_message_content(&pool).await.unwrap(), 1);
        assert_eq!(normalize_legacy_message_content(&pool).await.unwrap(), 0);

        let rows = sqlx::query("SELECT content, typeof(content) AS storage_type FROM messages")
            .fetch_all(&pool)
            .await
            .expect("normalized rows should load");
        let decoded = rows
            .iter()
            .map(|row| {
                assert_eq!(row.get::<String, _>("storage_type"), "blob");
                decode_message_content(row, "content").expect("normalized content should decode")
            })
            .collect::<Vec<_>>();

        assert_eq!(decoded, ["待迁移", "已压缩"]);
    }
}
