use super::cleanup_recovery_cleanup_debt_on_startup;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CACHE_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestCache(PathBuf);

impl TestCache {
    fn new() -> Self {
        let suffix = NEXT_CACHE_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "vcp-recovery-cleanup-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("sse_cache")).expect("创建恢复缓存目录失败");
        Self(root)
    }

    fn claimed_path(&self) -> PathBuf {
        self.0
            .join("sse_cache")
            .join("sse_recovered_startup.claimed.g7.e1.p1.s1")
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

async fn cleanup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建恢复清理测试数据库失败");
    sqlx::raw_sql(include_str!(
        "../../../migrations/0011_recovery_cleanup_outbox.sql"
    ))
    .execute(&pool)
    .await
    .expect("初始化恢复清理 outbox 失败");
    sqlx::query(
        "CREATE TABLE active_generations (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            helper_generation BIGINT
        )",
    )
    .execute(&pool)
    .await
    .expect("初始化活动 generation 测试表失败");
    pool
}

async fn insert_debt(pool: &SqlitePool, path: &Path) {
    sqlx::query(
        "INSERT INTO recovery_cleanup_outbox
            (claimed_path, owner_type, owner_id, topic_id, msg_id, helper_generation, created_at)
         VALUES (?, 'agent', 'owner', 'topic', 'message', 7, 1)",
    )
    .bind(path.to_string_lossy().as_ref())
    .execute(pool)
    .await
    .expect("写入恢复清理义务失败");
}

async fn debt_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM recovery_cleanup_outbox")
        .fetch_one(pool)
        .await
        .expect("读取恢复清理义务数量失败")
}

#[tokio::test]
async fn 启动清扫在活动记录为空时仍处理_outbox并可重复执行() {
    let cache = TestCache::new();
    let pool = cleanup_pool().await;
    let claimed = cache.claimed_path();
    std::fs::write(&claimed, b"cleanup").expect("创建 claimed 文件失败");
    insert_debt(&pool, &claimed).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .expect("读取活动记录数量失败"),
        0
    );

    cleanup_recovery_cleanup_debt_on_startup(cache.root(), &pool).await;
    cleanup_recovery_cleanup_debt_on_startup(cache.root(), &pool).await;

    assert!(!claimed.exists(), "成功清扫后 claimed 文件必须删除");
    assert_eq!(debt_count(&pool).await, 0, "成功清扫后 outbox 必须删除");
}

#[tokio::test]
async fn 启动清扫_unlink失败时保留欠账并支持下次重试() {
    let cache = TestCache::new();
    let pool = cleanup_pool().await;
    let claimed = cache.claimed_path();
    std::fs::create_dir(&claimed).expect("创建失败注入目录失败");
    insert_debt(&pool, &claimed).await;

    cleanup_recovery_cleanup_debt_on_startup(cache.root(), &pool).await;

    assert!(claimed.is_dir(), "unlink 失败时残留必须保留");
    assert_eq!(debt_count(&pool).await, 1, "unlink 失败时 outbox 必须保留");
    std::fs::remove_dir(&claimed).expect("移除失败注入目录失败");
    std::fs::write(&claimed, b"retry").expect("重试时创建 claimed 文件失败");
    cleanup_recovery_cleanup_debt_on_startup(cache.root(), &pool).await;

    assert!(!claimed.exists(), "下次启动重试应删除 claimed 文件");
    assert_eq!(debt_count(&pool).await, 0, "下次启动重试应删除 outbox");
}
