use super::{finalize_if_active_with_cleanup, RecoveryFinalization, RecoveryPayload};
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MIGRATIONS: [&str; 11] = [
    include_str!("../../../migrations/0001_create_initial_tables.sql"),
    include_str!("../../../migrations/0002_add_deleted_at_to_message_attachments.sql"),
    include_str!("../../../migrations/0003_create_messages_fts.sql"),
    include_str!("../../../migrations/0004_fix_fts_triggers.sql"),
    include_str!("../../../migrations/0005_ensure_active_generations.sql"),
    include_str!("../../../migrations/0006_add_render_cache_identity.sql"),
    include_str!("../../../migrations/0007_add_deleted_at_to_avatars.sql"),
    include_str!("../../../migrations/0008_adopt_composite_identity.sql"),
    include_str!("../../../migrations/0009_decouple_group_member_tags.sql"),
    include_str!("../../../migrations/0010_add_helper_generation_to_active_generations.sql"),
    include_str!("../../../migrations/0011_recovery_cleanup_outbox.sql"),
];

static NEXT_CACHE_DIRECTORY: AtomicU64 = AtomicU64::new(1);

fn test_key() -> MessageKey {
    MessageKey::new(TopicKey::new("agent", "owner-a", "topic-a"), "message-a")
}

async fn finalization_pool(generation: i64, finish_reason: Option<&str>) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建恢复终结测试数据库失败");
    for migration in MIGRATIONS {
        sqlx::raw_sql(migration)
            .execute(&pool)
            .await
            .expect("恢复终结测试 schema 初始化失败");
    }
    sqlx::query(
        "INSERT INTO agents (agent_id, name, model, updated_at)
         VALUES ('owner-a', '测试代理', 'model', 1);
         INSERT INTO topics (
            owner_type, owner_id, topic_id, title, created_at, updated_at,
            last_message_updated_at, locked, unread, unread_count, msg_count,
            config_hash, content_hash
         ) VALUES ('agent', 'owner-a', 'topic-a', '测试主题', 1, 1, 0, 0, 0, 0, 1, '', '');",
    )
    .execute(&pool)
    .await
    .expect("写入恢复终结主体失败");
    sqlx::query(
        "INSERT INTO messages (
            owner_type, owner_id, topic_id, msg_id, role, name, agent_id,
            content, timestamp, is_group_message, group_id, finish_reason,
            content_hash, created_at, updated_at
         ) VALUES (
            'agent', 'owner-a', 'topic-a', 'message-a', 'assistant', NULL,
            'owner-a', '原始消息', 1, 0, NULL, ?, '', 1, 1
         )",
    )
    .bind(finish_reason)
    .execute(&pool)
    .await
    .expect("写入恢复消息主体失败");
    sqlx::query(
        "INSERT INTO active_generations
            (owner_type, owner_id, topic_id, msg_id, created_at, helper_generation)
         VALUES ('agent', 'owner-a', 'topic-a', 'message-a', 1, ?)",
    )
    .bind(generation)
    .execute(&pool)
    .await
    .expect("写入活动 generation 主体失败");
    pool
}

struct TestCache(PathBuf);

impl TestCache {
    fn new() -> Self {
        let suffix = NEXT_CACHE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("vcp-finalize-{}-{suffix}", std::process::id()));
        std::fs::create_dir_all(root.join("sse_cache")).expect("创建恢复缓存目录失败");
        Self(root)
    }

    fn claimed_path(&self, generation: u64) -> PathBuf {
        self.0
            .join("sse_cache")
            .join(format!("sse_recovered_test.claimed.g{generation}.e1.p1.s1"))
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestCache {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn payload(content: &str, generation: u64) -> RecoveryPayload {
    RecoveryPayload {
        content: content.to_string(),
        finish_reason: Some("completed".to_string()),
        generation,
    }
}

async fn message_state(pool: &SqlitePool) -> (String, Option<String>) {
    let row = sqlx::query(
        "SELECT content, finish_reason FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'
           AND topic_id = 'topic-a' AND msg_id = 'message-a'",
    )
    .fetch_one(pool)
    .await
    .expect("读取恢复消息状态失败");
    (
        decode_message_content(&row, "content").expect("恢复消息正文应可解码"),
        row.try_get("finish_reason").expect("读取恢复消息终态失败"),
    )
}

async fn active_generation(pool: &SqlitePool) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT helper_generation FROM active_generations
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'
           AND topic_id = 'topic-a' AND msg_id = 'message-a'",
    )
    .fetch_optional(pool)
    .await
    .expect("读取活动 generation 状态失败")
}

#[tokio::test]
async fn 旧generation不能写消息或删除新活动行() {
    let pool = finalization_pool(8, None).await;
    let cache = TestCache::new();
    let before = message_state(&pool).await;
    let result = finalize_if_active_with_cleanup(
        &pool,
        &test_key(),
        &payload("旧恢复结果", 7),
        cache.root(),
        &cache.claimed_path(7),
    )
    .await
    .expect("generation mismatch should return a guarded result");
    assert!(matches!(result, RecoveryFinalization::Skipped));
    assert_eq!(active_generation(&pool).await, Some(8));
    assert_eq!(message_state(&pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recovery_cleanup_outbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn render写入失败时消息活动行和outbox全部回滚() {
    let pool = finalization_pool(7, None).await;
    sqlx::query(
        "CREATE TRIGGER fail_recovery_render
         BEFORE INSERT ON render_cache
         BEGIN SELECT RAISE(ABORT, '恢复 render 注入失败'); END",
    )
    .execute(&pool)
    .await
    .expect("安装恢复 render 故障注入失败");
    let cache = TestCache::new();
    let before = message_state(&pool).await;
    let result = finalize_if_active_with_cleanup(
        &pool,
        &test_key(),
        &payload("不应提交", 7),
        cache.root(),
        &cache.claimed_path(7),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(active_generation(&pool).await, Some(7));
    assert_eq!(message_state(&pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recovery_cleanup_outbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn 缺失render表时恢复终结事务不留下部分状态() {
    let pool = finalization_pool(7, None).await;
    sqlx::query("DROP TABLE render_cache")
        .execute(&pool)
        .await
        .expect("删除 render 表故障夹具失败");
    let cache = TestCache::new();
    let before = message_state(&pool).await;
    let result = finalize_if_active_with_cleanup(
        &pool,
        &test_key(),
        &payload("不应写入", 7),
        cache.root(),
        &cache.claimed_path(7),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(active_generation(&pool).await, Some(7));
    assert_eq!(message_state(&pool).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM recovery_cleanup_outbox")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn 消息写入活动删除和清理outbox同事务提交() {
    let pool = finalization_pool(7, None).await;
    let cache = TestCache::new();
    let claimed_path = cache.claimed_path(7);
    let result = finalize_if_active_with_cleanup(
        &pool,
        &test_key(),
        &payload("恢复后的正文", 7),
        cache.root(),
        &claimed_path,
    )
    .await
    .expect("恢复终结事务应提交");
    assert!(matches!(result, RecoveryFinalization::Applied));
    assert_eq!(active_generation(&pool).await, None);
    assert_eq!(
        message_state(&pool).await,
        ("恢复后的正文".to_string(), Some("completed".to_string()))
    );
    let outbox: (String, String, String, String, i64) = sqlx::query_as(
        "SELECT owner_type, owner_id, topic_id, msg_id, helper_generation
         FROM recovery_cleanup_outbox WHERE claimed_path = ?",
    )
    .bind(claimed_path.to_string_lossy().as_ref())
    .fetch_one(&pool)
    .await
    .expect("清理 outbox 应与恢复终态一起提交");
    assert_eq!(
        outbox,
        (
            "agent".to_string(),
            "owner-a".to_string(),
            "topic-a".to_string(),
            "message-a".to_string(),
            7,
        )
    );
}
