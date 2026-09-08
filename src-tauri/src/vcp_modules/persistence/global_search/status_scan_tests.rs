use super::super::{get_fts_index_status, status, status_scan};
use super::{
    assert_counts, 创建共享索引测试数据库, 创建索引测试数据库, 提交删除消息和索引,
    提交新增消息和索引, 插入测试消息, 插入测试索引行, 清理共享索引测试数据库,
};
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use sqlx::{Connection, Row, SqlitePool};
use std::time::Instant;
use tokio::time::{timeout, Duration};

async fn 查询计划(pool: &SqlitePool, sql: &str, continuation: bool) -> Vec<String> {
    let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
    let rows = if continuation {
        sqlx::query(&explain_sql)
            .bind(-1_i64)
            .bind(status_scan::STATUS_BATCH_SIZE)
            .fetch_all(pool)
            .await
    } else {
        sqlx::query(&explain_sql)
            .bind(status_scan::STATUS_BATCH_SIZE)
            .fetch_all(pool)
            .await
    };
    rows.expect("应读取查询计划")
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect()
}

#[tokio::test]
async fn 完整性批次查询按行号范围扫描且不建立临时排序() {
    let pool = 创建索引测试数据库(1).await;
    let live_plan = 查询计划(&pool, status_scan::LIVE_IDENTITY_BATCH_SQL, true).await;
    let fts_plan = 查询计划(&pool, status_scan::FTS_IDENTITY_BATCH_SQL, true).await;
    let stale_plan = 查询计划(&pool, status_scan::STALE_BATCH_SQL, true).await;
    let stale_first_plan = 查询计划(&pool, status_scan::STALE_FIRST_BATCH_SQL, false).await;

    assert!(
        live_plan
            .iter()
            .any(|detail| detail.contains("INTEGER PRIMARY KEY (rowid>?)")),
        "live identity query must use rowid range scan: {live_plan:?}"
    );
    assert!(
        fts_plan
            .iter()
            .any(|detail| detail.contains("VIRTUAL TABLE INDEX") && detail.contains(":>")),
        "FTS identity query must use rowid range scan: {fts_plan:?}"
    );
    assert!(
        stale_plan
            .iter()
            .any(|detail| detail.contains("VIRTUAL TABLE INDEX") && detail.contains(":>")),
        "stale query must use FTS rowid range scan: {stale_plan:?}"
    );
    assert!(
        stale_plan.iter().any(|detail| {
            detail.contains("SEARCH m USING INDEX sqlite_autoindex_messages_1")
                && detail.contains("owner_type=?")
        }),
        "stale query must probe messages through the composite primary key: {stale_plan:?}"
    );
    assert!(
        !stale_plan.iter().any(|detail| detail.starts_with("SCAN m")),
        "stale query must not rescan messages for every FTS row: {stale_plan:?}"
    );
    assert!(
        stale_first_plan
            .iter()
            .any(|detail| detail.contains("SCAN f VIRTUAL TABLE")),
        "stale first query must scan the FTS table as the outer input: {stale_first_plan:?}"
    );
    assert!(
        stale_first_plan.iter().any(|detail| {
            detail.contains("SEARCH m USING INDEX sqlite_autoindex_messages_1")
                && detail.contains("owner_type=?")
        }),
        "stale first query must probe messages through the composite primary key: {stale_first_plan:?}"
    );
    assert!(
        !stale_first_plan
            .iter()
            .any(|detail| detail.starts_with("SCAN m")),
        "stale first query must not rescan messages for every FTS row: {stale_first_plan:?}"
    );
    for plan in [&live_plan, &fts_plan, &stale_plan, &stale_first_plan] {
        assert!(
            !plan
                .iter()
                .any(|detail| detail.contains("USE TEMP B-TREE FOR ORDER BY")),
            "batch query must not sort into a temporary B-tree: {plan:?}"
        );
    }
}

#[tokio::test]
async fn 状态扫描跨连接提交保持同一旧快照() {
    let (pool, path) = 创建共享索引测试数据库().await;
    插入测试消息(&pool, "m1", "old body").await;
    插入测试索引行(&pool, "m1", "old body").await;

    let mut connection_a = pool.acquire().await.expect("应取得快照连接");
    let mut connection_b = pool.acquire().await.expect("应取得写入连接");
    let mut transaction_a = connection_a.begin().await.expect("应开始快照事务");
    status_scan::establish_snapshot(&mut *transaction_a)
        .await
        .expect("应建立状态扫描快照");

    提交新增消息和索引(&mut connection_b, "m2", "new body").await;
    let stale = status_scan::count_stale_rows(&mut *transaction_a)
        .await
        .expect("旧快照应可继续扫描过期状态");
    assert_eq!(stale.stale_count, 0);
    assert_eq!(stale.decode_error_count, 0);

    let status = status::status_in_connection(&mut *transaction_a)
        .await
        .expect("旧快照状态应可聚合");
    assert_counts(&status, [1, 1, 0, 0, 0, 0, 0, 1]);
    transaction_a.rollback().await.expect("应回滚快照事务");
    drop(connection_a);
    drop(connection_b);
    let fresh_status = get_fts_index_status(&pool)
        .await
        .expect("提交后新事务应读取完整状态");
    assert_counts(&fresh_status, [2, 2, 0, 0, 0, 0, 0, 1]);
    pool.close().await;
    清理共享索引测试数据库(&path);
}

#[tokio::test]
async fn 身份扫描跨页删除保持旧快照且无误报() {
    let (pool, path) = 创建共享索引测试数据库().await;
    for index in 0..300 {
        let id = format!("m{index:03}");
        插入测试消息(&pool, &id, "body").await;
        插入测试索引行(&pool, &id, "body").await;
    }

    let mut connection_a = pool.acquire().await.expect("应取得快照连接");
    let mut connection_b = pool.acquire().await.expect("应取得写入连接");
    let mut transaction_a = connection_a.begin().await.expect("应开始快照事务");
    status_scan::establish_snapshot(&mut *transaction_a)
        .await
        .expect("应建立身份扫描快照");
    let first_live = status_scan::load_identity_page(&mut *transaction_a, false, None)
        .await
        .expect("应读取消息首批");
    let first_fts = status_scan::load_identity_page(&mut *transaction_a, true, None)
        .await
        .expect("应读取索引首批");
    assert_eq!(first_live.len(), status_scan::STATUS_BATCH_SIZE as usize);
    assert_eq!(first_fts.len(), status_scan::STATUS_BATCH_SIZE as usize);
    let live_cursor = first_live
        .last()
        .expect("消息首批不应为空")
        .get::<i64, _>("message_rowid");
    let fts_cursor = first_fts
        .last()
        .expect("索引首批不应为空")
        .get::<i64, _>("fts_rowid");

    提交删除消息和索引(&mut connection_b, "m256").await;
    let live_continuation =
        status_scan::load_identity_page(&mut *transaction_a, false, Some(live_cursor))
            .await
            .expect("旧快照应读取消息续页");
    let fts_continuation =
        status_scan::load_identity_page(&mut *transaction_a, true, Some(fts_cursor))
            .await
            .expect("旧快照应读取索引续页");
    assert!(live_continuation
        .iter()
        .any(|row| row.get::<String, _>("msg_id") == "m256"));
    assert!(fts_continuation
        .iter()
        .any(|row| row.get::<String, _>("msg_id") == "m256"));

    let status = status::status_in_connection(&mut *transaction_a)
        .await
        .expect("旧快照身份状态应可聚合");
    assert_counts(&status, [300, 300, 0, 0, 0, 0, 0, 1]);
    transaction_a.rollback().await.expect("应回滚快照事务");
    drop(connection_a);
    drop(connection_b);
    pool.close().await;
    清理共享索引测试数据库(&path);
}

#[tokio::test]
async fn 状态扫描五万条消息保持可扩展() {
    const MESSAGE_COUNT: i64 = 50_000;
    let pool = 创建索引测试数据库(1).await;
    let compressed = ContentCompressor::compress("body").expect("应压缩批量测试正文");
    sqlx::query(
        "WITH RECURSIVE ids(n) AS (
             SELECT 1
             UNION ALL
             SELECT n + 1 FROM ids WHERE n < ?
         )
         INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content)
         SELECT 'agent', 'owner', 'topic', printf('m%05d', n), ?
         FROM ids",
    )
    .bind(MESSAGE_COUNT)
    .bind(compressed)
    .execute(&pool)
    .await
    .expect("应批量插入消息");
    sqlx::query(
        "WITH RECURSIVE ids(n) AS (
             SELECT 1
             UNION ALL
             SELECT n + 1 FROM ids WHERE n < ?
         )
         INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         SELECT printf('m%05d', n), 'topic', 'body', 'agent', 'owner'
         FROM ids",
    )
    .bind(MESSAGE_COUNT)
    .execute(&pool)
    .await
    .expect("应批量插入全文索引行");
    sqlx::query("UPDATE messages SET content = ? WHERE msg_id = 'm30000'")
        .bind(ContentCompressor::compress("stale body").expect("应压缩过期测试正文"))
        .execute(&pool)
        .await
        .expect("应制造批次末尾的过期索引行");

    let started = Instant::now();
    let status = timeout(Duration::from_secs(20), get_fts_index_status(&pool))
        .await
        .expect("五万条消息状态扫描应在时限内完成")
        .expect("应检查五万条消息索引状态");
    let elapsed = started.elapsed();
    assert_counts(&status, [MESSAGE_COUNT, MESSAGE_COUNT, 0, 0, 0, 1, 0, 0]);
    assert!(
        elapsed < Duration::from_secs(10),
        "状态扫描耗时过长，可能退化为复合 JOIN 重扫: {elapsed:?}"
    );
}

#[tokio::test]
async fn 状态扫描跨批次仅统计一次损坏消息解码错误() {
    let pool = 创建索引测试数据库(1).await;
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content)
         VALUES ('agent', 'owner', 'topic', 'bad', ?)",
    )
    .bind(vec![0x28_u8, 0xB5, 0x2F, 0xFD, 0xFF])
    .execute(&pool)
    .await
    .expect("应插入无效压缩消息");
    插入测试索引行(&pool, "bad", "old bad body").await;
    for index in 0..300 {
        let id = format!("normal-{index}");
        插入测试消息(&pool, &id, "body").await;
        插入测试索引行(&pool, &id, "body").await;
    }
    插入测试索引行(&pool, "bad", "old bad body").await;
    sqlx::query("UPDATE messages SET content = ? WHERE msg_id = 'normal-299'")
        .bind(ContentCompressor::compress("changed body").expect("应压缩过期测试正文"))
        .execute(&pool)
        .await
        .expect("应制造跨批次过期索引行");

    let status = get_fts_index_status(&pool).await.expect("应检查跨批次状态");
    assert_counts(&status, [301, 302, 0, 0, 1, 1, 1, 0]);
    assert_eq!(
        status.diagnostic.as_deref(),
        Some("FTS_CONTENT_DECODE_FAILED")
    );
}

#[tokio::test]
async fn 话题统计不依赖话题表且保留完整复合身份() {
    let pool = 创建索引测试数据库(1).await;
    sqlx::query("DROP TABLE topics")
        .execute(&pool)
        .await
        .expect("应允许最小状态 fixture 不创建话题表");
    for (owner_type, owner_id, topic_id, msg_id) in [
        ("a", "bc", "topic", "first"),
        ("ab", "c", "topic", "second"),
    ] {
        sqlx::query(
            "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, content)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(topic_id)
        .bind(msg_id)
        .bind(ContentCompressor::compress("body").expect("应压缩测试正文"))
        .execute(&pool)
        .await
        .expect("应插入复合身份消息");
        sqlx::query(
            "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
             VALUES (?, ?, 'body', ?, ?)",
        )
        .bind(msg_id)
        .bind(topic_id)
        .bind(owner_type)
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("应插入复合身份索引行");
    }

    let status = get_fts_index_status(&pool)
        .await
        .expect("最小状态 fixture 应可检查");
    assert_eq!(status.topic_count, 2);
    assert_counts(&status, [2, 2, 0, 0, 0, 0, 0, 1]);
}

#[tokio::test]
async fn 状态报告独立统计话题表中的存活行() {
    let pool = 创建索引测试数据库(1).await;
    for (owner_type, owner_id, topic_id, deleted_at) in [
        ("agent", "owner", "topic-a", None),
        ("agent", "owner", "topic-b", None),
        ("agent", "owner", "topic-deleted", Some(1_i64)),
    ] {
        sqlx::query(
            "INSERT INTO topics(owner_type, owner_id, topic_id, deleted_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(owner_type)
        .bind(owner_id)
        .bind(topic_id)
        .bind(deleted_at)
        .execute(&pool)
        .await
        .expect("应插入话题行");
    }

    let status = get_fts_index_status(&pool)
        .await
        .expect("应读取独立话题行计数");
    assert_eq!(status.topic_count, 0);
    assert_eq!(status.live_topic_row_count, 2);
}

#[tokio::test]
async fn 状态扫描不遗漏负行号记录() {
    let pool = 创建索引测试数据库(1).await;
    let compressed = ContentCompressor::compress("negative body").expect("应压缩负行号正文");
    sqlx::query(
        "INSERT INTO messages(rowid, owner_type, owner_id, topic_id, msg_id, content)
         VALUES (?, 'agent', 'owner', 'topic', 'negative', ?)",
    )
    .bind(i64::MIN)
    .bind(compressed)
    .execute(&pool)
    .await
    .expect("应插入负行号消息");
    sqlx::query(
        "INSERT INTO messages_fts(rowid, msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, 'negative', 'topic', 'negative body', 'agent', 'owner')",
    )
    .bind(i64::MIN)
    .execute(&pool)
    .await
    .expect("应插入负行号全文索引行");

    let status = get_fts_index_status(&pool)
        .await
        .expect("应检查负行号索引状态");
    assert_counts(&status, [1, 1, 0, 0, 0, 0, 0, 1]);
}
