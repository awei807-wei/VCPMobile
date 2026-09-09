use super::files::{read_recovery_payload, RecoveryFileClaim};
use super::finalize::{self, RecoveryPayload};
use crate::vcp_modules::chat::topic_types::MessageKey;

pub(super) enum ClaimedPayload {
    Ready(RecoveryPayload),
    Stale,
    NotFound,
}

pub(super) async fn load_claimed_payload(
    claim: &mut RecoveryFileClaim,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<ClaimedPayload, String> {
    let (content, timestamp, finish_reason, generation) =
        match read_recovery_payload(claim.claimed_path()) {
            Ok(payload) => payload,
            Err(error) => {
                log::warn!(
                "[VCPClient] 丢弃缺少真实 generation 的恢复文件：error={error}; result=discarded"
            );
                claim.commit()?;
                return Ok(ClaimedPayload::NotFound);
            }
        };
    if claim.generation() != Some(generation) {
        log::warn!(
            "[VCPClient] 恢复 claim 文件名 generation 与 payload 不一致，安全清理：payload_generation={generation}"
        );
        claim.commit()?;
        return Ok(ClaimedPayload::NotFound);
    }
    let age = chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(timestamp);
    if age > 24 * 3600 * 1000 {
        log::warn!("[VCPClient] 恢复文件超过 24 小时，删除后按未找到处理");
        claim.commit()?;
        return Ok(ClaimedPayload::NotFound);
    }
    let payload = RecoveryPayload {
        content,
        finish_reason,
        generation,
    };
    let Some(expected_generation) = finalize::active_helper_generation(pool, key).await? else {
        claim.commit()?;
        return Ok(ClaimedPayload::Stale);
    };
    if expected_generation != Some(payload.generation) {
        log::warn!(
            "[VCPClient] 恢复文件 generation 不匹配活动记录，安全清理：expected={expected_generation:?}, actual={}",
            payload.generation
        );
        claim.commit()?;
        return Ok(ClaimedPayload::NotFound);
    }
    Ok(ClaimedPayload::Ready(payload))
}
