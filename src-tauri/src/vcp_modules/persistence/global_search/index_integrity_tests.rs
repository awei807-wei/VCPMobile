use super::{get_fts_index_status, rebuild_messages_fts, FtsIndexStatus, FtsRebuildResult};
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use tokio::time::{timeout, Duration};

async fn 创建索引测试数据库(max_connections: u32) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect("sqlite::memory:")
        .await
        .expect("应打开索引完整性测试数据库");
    sqlx::raw_sql(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            content BLOB NOT NULL,
            timestamp INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE VIRTUAL TABLE messages_fts USING fts5(
            msg_id UNINDEXED,
            topic_id UNINDEXED,
            content,
            owner_type UNINDEXED,
            owner_id UNINDEXED,
            tokenize = 'trigram'
         );",
    )
    .execute(&pool)
    .await
    .expect("应创建索引完整性测试结构");
    pool
}

async fn 插入测试消息(pool: &SqlitePool, id: &str, content: &str) {
    let compressed = ContentCompressor::compress(content).expect("应压缩消息正文");
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content)
         VALUES ('agent', 'owner', 'topic', ?, ?)",
    )
    .bind(id)
    .bind(compressed)
    .execute(pool)
    .await
    .expect("应插入消息");
}

async fn 插入测试索引行(pool: &SqlitePool, id: &str, content: &str) {
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, 'topic', ?, 'agent', 'owner')",
    )
    .bind(id)
    .bind(content)
    .execute(pool)
    .await
    .expect("应插入全文索引行");
}

fn assert_counts(status: &FtsIndexStatus, expected: [i64; 8]) {
    assert_eq!(
        [
            status.live_count,
            status.indexed_count,
            status.missing_count,
            status.orphan_count,
            status.duplicate_count,
            status.stale_count,
            status.decode_error_count,
            status.healthy as i64,
        ],
        expected
    );
}

#[tokio::test]
async fn 状态报告缺失孤儿重复和过期索引行() {
    let pool = 创建索引测试数据库(1).await;
    插入测试消息(&pool, "m1", "alpha").await;
    插入测试消息(&pool, "m2", "beta").await;
    插入测试消息(&pool, "m3", "gamma").await;
    插入测试索引行(&pool, "m1", "alpha").await;
    插入测试索引行(&pool, "m3", "gamma").await;
    插入测试索引行(&pool, "m3", "gamma").await;
    插入测试索引行(&pool, "orphan", "orphan").await;
    sqlx::query(
        "UPDATE messages SET content = ?
         WHERE msg_id = 'm3'",
    )
    .bind(ContentCompressor::compress("delta").expect("应压缩替换正文"))
    .execute(&pool)
    .await
    .expect("应更新压缩消息正文");

    let status = get_fts_index_status(&pool).await.expect("应检查索引状态");
    assert!(status.available);
    assert!(status.schema_valid);
    assert!(status.tokenizer_valid);
    assert_counts(&status, [3, 4, 1, 1, 1, 2, 0, 0]);
    assert!(!status.healthy);
}

#[tokio::test]
async fn 重建读取压缩正文并返回脱敏统计() {
    let pool = 创建索引测试数据库(1).await;
    插入测试消息(&pool, "m1", "压缩正文").await;
    插入测试消息(&pool, "m2", "second body").await;
    插入测试索引行(&pool, "orphan", "old").await;

    let result: FtsRebuildResult = rebuild_messages_fts(&pool).await.expect("应重建全文索引");
    assert_eq!(result.indexed_count, 2);
    assert!(result.duration_ms < 10_000);
    let status = get_fts_index_status(&pool)
        .await
        .expect("应检查重建后的状态");
    assert_counts(&status, [2, 2, 0, 0, 0, 0, 0, 1]);
    let indexed: Vec<String> =
        sqlx::query_scalar("SELECT content FROM messages_fts ORDER BY msg_id")
            .fetch_all(&pool)
            .await
            .expect("应读取重建后的索引正文");
    assert!(indexed.contains(&"second body".to_string()));
    assert!(indexed.contains(&"压缩正文".to_string()));
}

#[tokio::test]
async fn 解码失败回滚并保留原有索引() {
    let pool = 创建索引测试数据库(1).await;
    插入测试消息(&pool, "good", "new body").await;
    插入测试索引行(&pool, "good", "old body").await;
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content)
         VALUES ('agent', 'owner', 'topic', 'bad', ?)",
    )
    .bind(vec![0x28_u8, 0xB5, 0x2F, 0xFD, 0xFF])
    .execute(&pool)
    .await
    .expect("应插入无效压缩消息");
    插入测试索引行(&pool, "bad", "old bad body").await;

    let error = rebuild_messages_fts(&pool)
        .await
        .expect_err("无效正文必须使重建失败");
    assert!(error.contains("CONTENT_DECODE_FAILED"));
    let rows = sqlx::query("SELECT msg_id, content FROM messages_fts ORDER BY msg_id")
        .fetch_all(&pool)
        .await
        .expect("应读取保留的全文索引行");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("msg_id"), "bad");
    assert_eq!(rows[0].get::<String, _>("content"), "old bad body");
    assert_eq!(rows[1].get::<String, _>("content"), "old body");
}

#[tokio::test]
async fn 并发重建调用等待进程锁() {
    let pool = 创建索引测试数据库(2).await;
    插入测试消息(&pool, "m1", "body").await;
    let _lock = super::rebuild_lock().lock().await;
    let task_pool = pool.clone();
    let task = tokio::spawn(async move { rebuild_messages_fts(&task_pool).await });
    assert!(timeout(Duration::from_millis(50), task).await.is_err());
    drop(_lock);
    let result = timeout(Duration::from_secs(5), async {
        rebuild_messages_fts(&pool).await
    })
    .await
    .expect("释放锁后重建应完成")
    .expect("重建应成功");
    assert_eq!(result.indexed_count, 1);
}

#[tokio::test]
async fn 缺失结构会报告不可用且可由显式重建修复() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("应打开空数据库");
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            content BLOB NOT NULL,
            timestamp INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .expect("应创建缺失全文索引的消息表");
    let status = get_fts_index_status(&pool).await.expect("应检查缺失结构");
    assert!(!status.available);
    assert!(!status.schema_valid);
    assert!(!status.tokenizer_valid);
    assert_eq!(status.diagnostic.as_deref(), Some("FTS_SCHEMA_UNAVAILABLE"));
    let result = rebuild_messages_fts(&pool)
        .await
        .expect("显式重建应创建缺失结构");
    assert_eq!(result.indexed_count, 0);
    let repaired = get_fts_index_status(&pool)
        .await
        .expect("应检查修复后的结构");
    assert!(repaired.available);
    assert!(repaired.schema_valid);
    assert!(repaired.tokenizer_valid);
    assert!(repaired.healthy);
}
