use super::files::claim_recovery_file_in_transition;
use super::{
    classify_helper_status, completed_recovery_result, finalize_recovery_claim,
    handle_recovery_attempt, helper_query_failed, read_recovery_payload,
    stable_stream_identity_token, validate_helper_identity, GuardedTransition, HelperStatus,
    RecoveryAttempt, RecoveryError, RecoveryErrorKind, RecoveryFileClaim, RecoveryFinalization,
};
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::infra::vcp_client::ActiveRequestRegistry;
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::oneshot;

async fn claim_recovery_file(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &std::path::Path,
    msg_id: &str,
    lease: &super::CompletionLease,
) -> Result<GuardedTransition<Option<super::RecoveryFileClaim>>, String> {
    let pool = pool.clone();
    let cache_dir = cache_dir.to_path_buf();
    let msg_id = msg_id.to_string();
    let epoch = lease.epoch();
    lease
        .with_current_transition(|current_key| async move {
            claim_recovery_file_in_transition(&pool, &cache_dir, &current_key, &msg_id, epoch).await
        })
        .await
}

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

fn test_key() -> MessageKey {
    MessageKey::new(TopicKey::new("agent", "owner-a", "topic-a"), "message-a")
}

async fn helper_generation_pool(key: &MessageKey) -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建 helper generation 测试数据库失败");
    sqlx::query(
        "CREATE TABLE active_generations (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            helper_generation INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .expect("创建活动 generation 表失败");
    sqlx::query(
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            content TEXT NOT NULL,
            finish_reason TEXT,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
        )",
    )
    .execute(&pool)
    .await
    .expect("创建消息测试表失败");
    sqlx::query(
        "INSERT INTO messages
            (owner_type, owner_id, topic_id, msg_id, content, finish_reason)
         VALUES (?, ?, ?, ?, '原始消息', NULL)",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(&pool)
    .await
    .expect("写入消息测试行失败");
    pool
}

async fn insert_active_generation(
    pool: &SqlitePool,
    key: &MessageKey,
    generation: Option<i64>,
    finish_reason: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO active_generations
            (owner_type, owner_id, topic_id, msg_id, created_at, helper_generation)
         VALUES (?, ?, ?, ?, 1, ?)",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(generation)
    .execute(pool)
    .await
    .expect("写入活动 generation 测试行失败");
    sqlx::query(
        "UPDATE messages SET finish_reason = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(finish_reason)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(pool)
    .await
    .expect("更新消息终态测试行失败");
}

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let suffix = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("vcp-recovery-{}-{suffix}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).expect("创建恢复测试目录失败");
        std::fs::create_dir(root.join("sse_cache")).expect("创建恢复缓存目录失败");
        Self(root)
    }

    fn cache_dir(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn helper瞬时错误保持活动租约() {
    let registry = ActiveRequestRegistry::default();
    let key = test_key();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };
    let error = RecoveryError::retryable("helper TCP 连接失败");
    assert_eq!(error.kind, RecoveryErrorKind::Retryable);
    let result = handle_recovery_attempt(helper_query_failed(error));
    assert!(result.unwrap_err().contains("可重试"));
    assert!(registry.contains_key(&key));
    drop(lease);
    assert!(!registry.contains_key(&key));
}

#[test]
fn 只有完整身份未找到状态才允许失败回退() {
    let key = test_key();
    let valid_identity = json!({
        "requestId": "message-a",
        "messageId": "message-a",
        "ownerType": "agent",
        "ownerId": "owner-a",
        "topicId": "topic-a",
        "status": "not_found",
        "generation": null
    });
    assert!(validate_helper_identity(&valid_identity, &key, "message-a").is_ok());
    assert_eq!(
        classify_helper_status(&valid_identity).unwrap(),
        HelperStatus::NotFound
    );
    assert_eq!(
        super::validate_helper_status_generation(&valid_identity, 61).unwrap(),
        HelperStatus::NotFound
    );
    let missing_identity = json!({"status": "not_found"});
    assert!(validate_helper_identity(&missing_identity, &key, "message-a").is_err());
    let invalid = classify_helper_status(&json!({"status": "暂时不可用"}));
    assert!(matches!(
        invalid,
        Err(RecoveryError {
            kind: RecoveryErrorKind::Infrastructure,
            ..
        })
    ));
    assert!(handle_recovery_attempt(RecoveryAttempt::NotFound)
        .unwrap()
        .is_none());
}

#[test]
fn helper_not_found的null_generation不参与generation校验() {
    let response = json!({
        "status": "not_found",
        "generation": null
    });
    assert_eq!(
        super::validate_helper_status_generation(&response, 61).unwrap(),
        HelperStatus::NotFound
    );
}

#[tokio::test]
async fn 占用恢复文件后新文件不会被旧清理删除() {
    let directory = TestDirectory::new();
    let key = test_key();
    let file = directory.cache_dir().join("sse_cache").join(format!(
        "sse_recovered_{}.json",
        stable_stream_identity_token(&key)
    ));
    std::fs::write(&file, r#"{"content":"A","timestamp":1,"generation":7}"#)
        .expect("写入旧恢复文件失败");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let registry = ActiveRequestRegistry::default();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };
    let mut claimed = match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
        .await
        .unwrap()
    {
        GuardedTransition::Applied(Some(claim)) => claim,
        GuardedTransition::Applied(None) => panic!("应占用旧恢复文件"),
        GuardedTransition::Skipped => panic!("当前恢复租约不应过期"),
    };
    assert!(!file.exists());
    assert!(claimed.claimed_path().exists());
    assert!(claimed
        .claimed_path()
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".claimed.g7.")));
    assert_eq!(claimed.generation(), Some(7));

    let (_, _previous_sender, new_lease) =
        registry.register(key.clone(), oneshot::channel().0).await;
    std::fs::write(&file, r#"{"content":"B","timestamp":2,"generation":8}"#)
        .expect("写入新恢复文件失败");
    claimed.commit().expect("清理旧恢复文件失败");
    assert_eq!(
        std::fs::read_to_string(&file).expect("读取新恢复文件失败"),
        r#"{"content":"B","timestamp":2,"generation":8}"#
    );
    drop(lease);
    assert!(registry.contains_key(&key));
    assert!(registry.remove_key_guarded(&key).await.is_some());
    drop(new_lease);
    assert!(!registry.contains_key(&key));
    drop(pool);
}

#[tokio::test]
async fn 首次恢复注入失败回滚后第二次可以发现并提交() {
    let directory = TestDirectory::new();
    let key = test_key();
    let file = directory.cache_dir().join("sse_cache").join(format!(
        "sse_recovered_{}.json",
        stable_stream_identity_token(&key)
    ));
    std::fs::write(&file, r#"{"content":"损坏","timestamp":"不是数字"}"#)
        .expect("写入损坏恢复文件失败");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let registry = ActiveRequestRegistry::default();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };

    let mut first_claim =
        match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
            .await
            .unwrap()
        {
            GuardedTransition::Applied(Some(claim)) => claim,
            GuardedTransition::Applied(None) => panic!("首次恢复应找到文件"),
            GuardedTransition::Skipped => panic!("首次恢复租约不应过期"),
        };
    assert!(read_recovery_payload(first_claim.claimed_path()).is_err());
    first_claim.rollback().expect("首次恢复失败应回滚文件");
    assert!(file.exists());

    std::fs::write(
        &file,
        format!(
            r#"{{"content":"第二次成功","timestamp":{},"generation":7}}"#,
            chrono::Utc::now().timestamp_millis()
        ),
    )
    .expect("写入可恢复文件失败");
    let mut second_claim =
        match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
            .await
            .unwrap()
        {
            GuardedTransition::Applied(Some(claim)) => claim,
            GuardedTransition::Applied(None) => panic!("第二次恢复应重新发现文件"),
            GuardedTransition::Skipped => panic!("第二次恢复租约不应过期"),
        };
    let (content, _, _, _) = read_recovery_payload(second_claim.claimed_path()).unwrap();
    assert_eq!(content, "第二次成功");
    second_claim.commit().expect("第二次恢复提交失败");
    assert!(!file.exists());
    drop(lease);
    drop(pool);
}

#[tokio::test]
async fn 首次恢复数据库终结失败回滚后第二次发现并成功() {
    let directory = TestDirectory::new();
    let key = test_key();
    let file = directory.cache_dir().join("sse_cache").join(format!(
        "sse_recovered_{}.json",
        stable_stream_identity_token(&key)
    ));
    std::fs::write(
        &file,
        format!(
            r#"{{"content":"首次数据库失败","timestamp":{},"generation":7}}"#,
            chrono::Utc::now().timestamp_millis()
        ),
    )
    .expect("写入恢复文件失败");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let registry = ActiveRequestRegistry::default();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };
    let first_claim = match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
        .await
        .unwrap()
    {
        GuardedTransition::Applied(Some(claim)) => claim,
        GuardedTransition::Applied(None) => panic!("首次恢复应找到文件"),
        GuardedTransition::Skipped => panic!("首次恢复租约不应过期"),
    };
    let first_result = finalize_recovery_claim(first_claim, || async {
        sqlx::query("INSERT INTO missing_recovery_table (content) VALUES (?)")
            .bind("失败注入")
            .execute(&pool)
            .await
            .map(|_| RecoveryFinalization::Applied)
            .map_err(|error| error.to_string())
    })
    .await;
    assert!(first_result.is_err());
    assert!(file.exists(), "数据库终结失败后必须恢复原文件");

    let second_claim = match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
        .await
        .unwrap()
    {
        GuardedTransition::Applied(Some(claim)) => claim,
        GuardedTransition::Applied(None) => panic!("第二次恢复应重新发现文件"),
        GuardedTransition::Skipped => panic!("第二次恢复租约不应过期"),
    };
    let (content, _, _, _) = read_recovery_payload(second_claim.claimed_path()).unwrap();
    assert_eq!(content, "首次数据库失败");
    finalize_recovery_claim(second_claim, || async {
        Ok::<RecoveryFinalization, String>(RecoveryFinalization::Applied)
    })
    .await
    .expect("第二次恢复应提交成功");
    assert!(!file.exists());
    drop(lease);
    drop(pool);
}

#[test]
fn 恢复令牌与kotlin固定样例一致() {
    let key = MessageKey::new(TopicKey::new("group", "owner/1", "topic:1"), "message-1");
    assert_eq!(
        stable_stream_identity_token(&key),
        "082b896ff27af4a02a8feb019daf3dd40fded7e590357528de4116da79d8dec6"
    );
}

#[test]
fn 恢复令牌使用utf8字节长度而不是字符长度() {
    let key = MessageKey::new(TopicKey::new("group", "所有者", "主题"), "消息-1");
    assert_eq!(
        stable_stream_identity_token(&key),
        "b3c26bd54e7ad97c029f2659f12cd4e3eb1f4cee5caa19a03436ca332c04432b"
    );
}

#[test]
fn 恢复结果单独携带真实helper_generation() {
    let payload = super::RecoveryPayload {
        content: "已恢复".to_string(),
        finish_reason: Some("completed".to_string()),
        generation: 7,
    };
    let result = completed_recovery_result(&payload);
    assert_eq!(result["status"], "completed");
    assert_eq!(result["helperGeneration"], 7);
    assert!(result.get("generation").is_none());
}

#[tokio::test]
async fn 进程退出遗留的完整身份claim可以安全接管() {
    let directory = TestDirectory::new();
    let key = test_key();
    let claimed_file = directory.cache_dir().join("sse_cache").join(format!(
        "sse_recovered_{}.claimed.g7.e1.p123.s1",
        stable_stream_identity_token(&key)
    ));
    std::fs::write(
        &claimed_file,
        r#"{"content":"遗留","timestamp":1,"generation":7}"#,
    )
    .expect("写入遗留恢复文件失败");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let registry = ActiveRequestRegistry::default();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };
    let mut claim = match claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
        .await
        .unwrap()
    {
        GuardedTransition::Applied(Some(claim)) => claim,
        GuardedTransition::Applied(None) => panic!("应接管遗留 claim 文件"),
        GuardedTransition::Skipped => panic!("恢复租约不应过期"),
    };
    assert_eq!(claim.claimed_path(), claimed_file.as_path());
    claim.commit().expect("遗留 claim 清理失败");
    assert!(!claimed_file.exists());
}

#[tokio::test]
async fn 缺少generation的旧claim不会被猜测接管() {
    let directory = TestDirectory::new();
    let key = test_key();
    let old_claim = directory.cache_dir().join("sse_cache").join(format!(
        "sse_recovered_{}.claimed.旧进程.7",
        stable_stream_identity_token(&key)
    ));
    std::fs::write(&old_claim, r#"{"content":"旧","timestamp":1}"#).expect("写入旧 claim 失败");
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let registry = ActiveRequestRegistry::default();
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => lease,
        crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
            panic!("恢复租约不应已被占用")
        }
    };
    let result = claim_recovery_file(&pool, directory.cache_dir(), &key.msg_id, &lease)
        .await
        .unwrap();
    assert!(matches!(result, GuardedTransition::Applied(None)));
    assert!(!old_claim.exists());
    drop(lease);
    drop(pool);
}

#[tokio::test]
async fn 多个generation_claim按确定性规则拒绝恢复() {
    let directory = TestDirectory::new();
    let key = test_key();
    let token = stable_stream_identity_token(&key);
    for (generation, suffix) in [(7, 1), (8, 2)] {
        let path = directory.cache_dir().join("sse_cache").join(format!(
            "sse_recovered_{token}.claimed.g{generation}.e1.p123.s{suffix}"
        ));
        std::fs::write(
            path,
            format!(r#"{{"content":"{generation}","timestamp":1,"generation":{generation}}}"#),
        )
        .expect("写入重复 claim 失败");
    }
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建测试数据库失败");
    let result =
        super::files::find_recovery_file(&pool, directory.cache_dir(), &key, &key.msg_id).await;
    assert!(result.is_err());
    assert!(!directory
        .cache_dir()
        .join("sse_cache")
        .read_dir()
        .unwrap()
        .any(|entry| entry.unwrap().path().is_file()));
}

#[test]
fn 回滚遇到新canonical时不覆盖新内容() {
    let directory = TestDirectory::new();
    let original = directory
        .cache_dir()
        .join("sse_cache")
        .join("canonical.json");
    let claimed = directory
        .cache_dir()
        .join("sse_cache")
        .join("canonical.claimed.old");
    std::fs::write(&original, "B").expect("写入新 canonical 失败");
    std::fs::write(&claimed, "A").expect("写入旧 claim 失败");
    let mut file_claim = RecoveryFileClaim::new(original.clone(), claimed.clone());
    file_claim.rollback().expect("回滚不应失败");
    assert_eq!(std::fs::read_to_string(original).unwrap(), "B");
    assert_eq!(std::fs::read_to_string(claimed).unwrap(), "A");
}

#[test]
fn 数据库已提交但claim清理失败时记录清理欠账() {
    let directory = TestDirectory::new();
    let original = directory
        .cache_dir()
        .join("sse_cache")
        .join("canonical.json");
    let claimed = directory.cache_dir().join("sse_cache").join("claimed-dir");
    std::fs::create_dir(&claimed).expect("创建清理失败注入目录失败");
    let mut file_claim = RecoveryFileClaim::new(original, claimed.clone());
    file_claim.commit().expect("清理失败不能回滚数据库提交");
    assert!(file_claim.cleanup_pending());
    std::fs::remove_dir(&claimed).expect("清理测试目录失败");
}

#[tokio::test]
async fn 活动记录缺失时helper_generation校验拒绝恢复() {
    let key = test_key();
    let pool = helper_generation_pool(&key).await;
    let error = super::ensure_active_helper_generation(&pool, &key, 61)
        .await
        .expect_err("缺失活动记录必须拒绝恢复");
    assert!(error.contains("不存在"));
}

#[tokio::test]
async fn 活动记录为null时helper_generation校验拒绝恢复() {
    let key = test_key();
    let pool = helper_generation_pool(&key).await;
    insert_active_generation(&pool, &key, None, None).await;
    let error = super::ensure_active_helper_generation(&pool, &key, 61)
        .await
        .expect_err("NULL helper generation 必须拒绝恢复");
    assert!(error.contains("缺少 helper generation"));
}

#[tokio::test]
async fn 活动记录generation不一致时校验拒绝恢复() {
    let key = test_key();
    let pool = helper_generation_pool(&key).await;
    insert_active_generation(&pool, &key, Some(60), None).await;
    let error = super::ensure_active_helper_generation(&pool, &key, 61)
        .await
        .expect_err("不同 helper generation 必须拒绝恢复");
    assert!(error.contains("expected=61") && error.contains("actual=60"));
}

#[tokio::test]
async fn observed_generation_not_found时只清理匹配的活动记录() {
    let key = test_key();
    let pool = helper_generation_pool(&key).await;
    insert_active_generation(&pool, &key, Some(61), None).await;

    assert!(
        super::finalize::delete_active_generation_if_observed(&pool, &key, Some(61))
            .await
            .expect("按 observed generation 清理活动记录不应失败")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    drop(pool);
}

#[tokio::test]
async fn 终态和null活动记录自愈时不修改消息() {
    let key = test_key();
    for (generation, finish_reason) in [(None, None), (Some(61), Some("completed"))] {
        let pool = helper_generation_pool(&key).await;
        insert_active_generation(&pool, &key, generation, finish_reason).await;
        let app = tauri::test::mock_app();
        let registry = ActiveRequestRegistry::default();
        let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
            crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Claimed(lease) => {
                lease
            }
            crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive::Active { .. } => {
                panic!("测试恢复租约不应已被占用")
            }
        };
        let result = super::finalize_missing_generation(app.handle(), &pool, &lease, &key.msg_id)
            .await
            .expect("终态/NULL 自愈不应失败");
        assert_eq!(result["status"], "stale");
        let retry = super::finalize_missing_generation(app.handle(), &pool, &lease, &key.msg_id)
            .await
            .expect("重复恢复应幂等返回 stale");
        assert_eq!(retry["status"], "stale");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        let message: (String, Option<String>) = sqlx::query_as(
            "SELECT content, finish_reason FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(message.0, "原始消息");
        assert_eq!(message.1.as_deref(), finish_reason);
        drop(lease);
        drop(pool);
    }
}
