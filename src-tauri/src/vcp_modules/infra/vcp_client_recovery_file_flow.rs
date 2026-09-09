use super::files::{
    claim_recovery_file_in_transition as claim_file_in_transition, RecoveryFileClaim,
};
use super::finalize;
use super::payload::{load_claimed_payload, ClaimedPayload};
use super::{RecoveryAttempt, RecoveryFinalization, RecoveryPayload};
use crate::vcp_modules::chat::topic_types::MessageKey;
use crate::vcp_modules::infra::vcp_client::{CompletionLease, GuardedTransition};
use std::path::Path;

pub(super) async fn recover_from_disk(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &Path,
    msg_id: &str,
    recovery_lease: &CompletionLease,
) -> Result<RecoveryAttempt, String> {
    let pool = pool.clone();
    let cache_dir = cache_dir.to_path_buf();
    let msg_id = msg_id.to_string();
    let epoch = recovery_lease.epoch();
    let transition = recovery_lease
        .with_current_transition(|key| async move {
            recover_claimed_file(&pool, &cache_dir, &key, &msg_id, epoch).await
        })
        .await?;
    Ok(match transition {
        GuardedTransition::Applied(attempt) => attempt,
        GuardedTransition::Skipped => RecoveryAttempt::Stale,
    })
}

async fn recover_claimed_file(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &Path,
    key: &MessageKey,
    msg_id: &str,
    epoch: u64,
) -> Result<RecoveryAttempt, String> {
    let Some(mut claim) = claim_file_in_transition(pool, cache_dir, key, msg_id, epoch).await?
    else {
        return Ok(RecoveryAttempt::NotFound);
    };
    log::info!("[VCPClient] 找到完整身份对应的本地恢复文件：result=found");
    let payload = match load_claimed_payload(&mut claim, pool, key).await? {
        ClaimedPayload::Ready(payload) => payload,
        ClaimedPayload::Stale => return Ok(RecoveryAttempt::Stale),
        ClaimedPayload::NotFound => return Ok(RecoveryAttempt::NotFound),
    };
    let cleanup_path = claim.claimed_path().to_path_buf();
    let finalization = finalize_recovery_claim_with_pool(pool, claim, || async {
        finalize::finalize_if_active_with_cleanup(pool, key, &payload, cache_dir, &cleanup_path)
            .await
    })
    .await?;
    Ok(match finalization {
        RecoveryFinalization::Applied => {
            RecoveryAttempt::Completed(completed_recovery_result(&payload))
        }
        RecoveryFinalization::Skipped => RecoveryAttempt::Stale,
        RecoveryFinalization::NotFound => RecoveryAttempt::NotFound,
    })
}

pub(super) fn completed_recovery_result(payload: &RecoveryPayload) -> serde_json::Value {
    let mut result = serde_json::json!({
        "status": "completed",
        "content": payload.content,
    });
    result["helperGeneration"] = serde_json::json!(payload.generation);
    result
}

pub(super) async fn finalize_recovery_claim_with_pool<T, F, Fut>(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    claim: RecoveryFileClaim,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut claim = claim;
    match operation().await {
        Ok(result) => {
            claim.commit_with_pool(pool).await?;
            Ok(result)
        }
        Err(error) => {
            claim.rollback().map_err(|rollback_error| {
                format!("{error}; 恢复原恢复文件失败: {rollback_error}")
            })?;
            Err(error)
        }
    }
}

pub(super) async fn finalize_recovery_claim<T, F, Fut>(
    claim: RecoveryFileClaim,
    operation: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut claim = claim;
    match operation().await {
        Ok(result) => {
            claim.commit()?;
            Ok(result)
        }
        Err(error) => {
            claim.rollback().map_err(|rollback_error| {
                format!("{error}; 恢复原恢复文件失败: {rollback_error}")
            })?;
            Err(error)
        }
    }
}
