use super::super::{
    active::mark_message_as_error_guarded_with_generation, db_pool_if_ready, registry,
    ActiveRequests, CompletionLease, GuardedTransition,
};
use super::{legacy_recovery_file_is_unambiguous, resolve_legacy_generation_key};
use crate::vcp_modules::chat::topic_types::MessageKey;
use files::cleanup_recovery_cleanup_debt;
pub(crate) use files::cleanup_recovery_cleanup_debt_on_startup;
#[cfg(test)]
use files::read_recovery_payload;
#[cfg(test)]
use files::stable_stream_identity_token;
use finalize::RecoveryPayload;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

#[path = "vcp_client_recovery_files.rs"]
mod files;
#[path = "vcp_client_recovery_finalize.rs"]
mod finalize;

#[cfg(target_os = "android")]
#[path = "vcp_client_recovery_helper.rs"]
mod helper;

#[path = "vcp_client_recovery_file_flow.rs"]
mod file_flow;
#[path = "vcp_client_recovery_payload.rs"]
mod payload;

use file_flow::recover_from_disk;
#[cfg(test)]
use file_flow::{completed_recovery_result, finalize_recovery_claim};
#[cfg(test)]
use files::RecoveryFileClaim;

enum RecoveryAttempt {
    NotFound,
    Stale,
    Completed(Value),
    Streaming(Value),
    HelperError(RecoveryError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryErrorKind {
    Retryable,
    Infrastructure,
}

#[derive(Debug)]
struct RecoveryError {
    kind: RecoveryErrorKind,
    message: String,
}

impl RecoveryError {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: RecoveryErrorKind::Retryable,
            message: message.into(),
        }
    }

    fn infrastructure(message: impl Into<String>) -> Self {
        Self {
            kind: RecoveryErrorKind::Infrastructure,
            message: message.into(),
        }
    }

    fn into_message(self) -> String {
        let category = match self.kind {
            RecoveryErrorKind::Retryable => "可重试",
            RecoveryErrorKind::Infrastructure => "基础设施",
        };
        format!("helper 恢复{category}错误: {}", self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperStatus {
    NotFound,
    Completed,
    Streaming,
}

fn classify_helper_status(response: &Value) -> Result<HelperStatus, RecoveryError> {
    match response.get("status").and_then(Value::as_str) {
        Some("not_found") => Ok(HelperStatus::NotFound),
        Some("completed") => Ok(HelperStatus::Completed),
        Some("streaming") => Ok(HelperStatus::Streaming),
        Some(status) => Err(RecoveryError::infrastructure(format!(
            "helper 返回未知会话状态: {status}"
        ))),
        None => Err(RecoveryError::infrastructure("helper 响应缺少会话状态")),
    }
}

/// Missing helper sessions are authoritative absence, not a generation-bearing
/// session. A real `not_found` response carries `generation: null`, so it must
/// be returned before generation validation.
fn validate_helper_status_generation(
    response: &Value,
    expected_generation: u64,
) -> Result<HelperStatus, String> {
    let status = classify_helper_status(response).map_err(RecoveryError::into_message)?;
    if status == HelperStatus::NotFound {
        return Ok(status);
    }
    verify_helper_generation(response, expected_generation)?;
    Ok(status)
}

fn verify_helper_generation(response: &Value, expected_generation: u64) -> Result<(), String> {
    let actual = response["generation"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| "helper 查询响应缺少有效 generation".to_string())?;
    if actual != expected_generation {
        return Err(format!(
            "helper 查询 generation 不匹配: expected={expected_generation}, actual={actual}"
        ));
    }
    Ok(())
}

fn validate_helper_identity(
    response: &Value,
    key: &MessageKey,
    msg_id: &str,
) -> Result<(), RecoveryError> {
    if key.msg_id != msg_id {
        return Err(RecoveryError::infrastructure(
            "helper 查询的 requestId 与已验证 messageId 不一致",
        ));
    }
    for (field, expected) in [
        ("requestId", msg_id),
        ("messageId", msg_id),
        ("ownerType", key.topic.owner_type.as_str()),
        ("ownerId", key.topic.owner_id.as_str()),
        ("topicId", key.topic.topic_id.as_str()),
    ] {
        if response[field].as_str() != Some(expected) {
            return Err(RecoveryError::infrastructure(format!(
                "helper 查询响应身份字段 {field} 不一致"
            )));
        }
    }
    Ok(())
}

fn helper_query_failed(error: RecoveryError) -> RecoveryAttempt {
    log::warn!(
        "[VCPClient] helper 查询失败（分类={:?}），保留活动生成：result=pending",
        error.kind
    );
    RecoveryAttempt::HelperError(error)
}

enum RecoveryFinalization {
    Applied,
    Skipped,
    NotFound,
}

#[tauri::command]
pub async fn recover_active_generation<R: Runtime>(
    app: AppHandle<R>,
    active_requests: tauri::State<'_, ActiveRequests>,
    msg_id: String,
    owner_id: Option<String>,
    owner_type: Option<String>,
    topic_id: Option<String>,
) -> Result<Value, String> {
    let explicit_key = registry::optional_message_key(&msg_id, owner_id, owner_type, topic_id)?;
    log::info!(
        "[VCPClient] 收到恢复请求：已限定身份={}",
        explicit_key.is_some()
    );
    let pool = db_pool_if_ready(&app)?;
    let request_key = resolve_recovery_key(&pool, &msg_id, explicit_key).await?;
    recover_generation_for_key(&app, &pool, &active_requests.0, request_key, &msg_id).await
}

async fn resolve_recovery_key(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    msg_id: &str,
    explicit_key: Option<MessageKey>,
) -> Result<MessageKey, String> {
    match explicit_key {
        Some(key) => Ok(key),
        None => resolve_legacy_generation_key(pool, msg_id).await,
    }
}

async fn recover_generation_for_key<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    active_requests: &std::sync::Arc<super::super::ActiveRequestRegistry>,
    request_key: MessageKey,
    msg_id: &str,
) -> Result<Value, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?;
    cleanup_recovery_cleanup_debt(&cache_dir, pool).await?;
    let recovery_lease = match active_requests
        .claim_if_inactive(request_key.clone())
        .await?
    {
        registry::ClaimIfInactive::Active { generation, epoch } => {
            let Some(generation) = generation.filter(|value| *value > 0) else {
                return Err("活动请求缺少真实 helper generation".to_string());
            };
            #[cfg(target_os = "android")]
            return recover_active_android(
                app,
                pool,
                active_requests,
                &request_key,
                msg_id,
                generation,
                epoch,
            )
            .await;
            #[cfg(not(target_os = "android"))]
            return recover_active_desktop(generation, epoch).await;
        }
        registry::ClaimIfInactive::Claimed(lease) => lease,
    };
    recover_claimed_generation(app, pool, &cache_dir, &request_key, msg_id, &recovery_lease).await
}

#[cfg(target_os = "android")]
async fn recover_active_android<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    active_requests: &std::sync::Arc<super::super::ActiveRequestRegistry>,
    request_key: &MessageKey,
    msg_id: &str,
    generation: u64,
    epoch: u64,
) -> Result<Value, String> {
    let query_pool = pool.clone();
    let query_key = request_key.clone();
    let guarded = active_requests
        .with_active_generation(request_key, epoch, generation, || async {
            ensure_active_helper_generation(&query_pool, &query_key, generation).await?;
            let attempt =
                helper::query_active_helper_session(app, &query_key, msg_id, generation).await?;
            ensure_active_helper_generation(&query_pool, &query_key, generation).await?;
            if matches!(&attempt, RecoveryAttempt::Stale | RecoveryAttempt::NotFound) {
                let removed = finalize::delete_active_generation_if_observed(
                    &query_pool,
                    &query_key,
                    Some(generation),
                )
                .await?;
                if !removed {
                    return Err(
                        "helper 终态自愈时活动 generation 已被替换，拒绝继续恢复".to_string()
                    );
                }
            }
            Ok(if matches!(attempt, RecoveryAttempt::NotFound) {
                RecoveryAttempt::Stale
            } else {
                attempt
            })
        })
        .await?;
    match guarded {
        GuardedTransition::Skipped => Ok(json!({ "status": "stale" })),
        GuardedTransition::Applied(attempt) => match attempt {
            RecoveryAttempt::Streaming(result) => Ok(result),
            RecoveryAttempt::HelperError(error) => Err(error.into_message()),
            RecoveryAttempt::Completed(_) | RecoveryAttempt::Stale | RecoveryAttempt::NotFound => {
                Ok(json!({ "status": "stale" }))
            }
        },
    }
}

#[cfg(not(target_os = "android"))]
async fn recover_active_desktop(generation: u64, epoch: u64) -> Result<Value, String> {
    log::info!("[VCPClient] 活动生成仍在运行，返回真实 session generation");
    Ok(json!({
        "status": "streaming",
        "helperGeneration": generation,
        "requestEpoch": epoch
    }))
}

async fn recover_claimed_generation<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &std::path::Path,
    _request_key: &MessageKey,
    msg_id: &str,
    recovery_lease: &CompletionLease,
) -> Result<Value, String> {
    #[cfg(target_os = "android")]
    if let Some(result) = handle_recovery_attempt(
        helper::recover_from_helper(app, pool, _request_key, msg_id, recovery_lease).await?,
    )? {
        return Ok(result);
    }
    if let Some(result) =
        handle_recovery_attempt(recover_from_disk(pool, cache_dir, msg_id, recovery_lease).await?)?
    {
        return Ok(result);
    }
    finalize_missing_generation(app, pool, recovery_lease, msg_id).await
}

async fn ensure_active_helper_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    expected_generation: u64,
) -> Result<(), String> {
    match finalize::active_helper_generation(pool, key).await? {
        Some(Some(actual)) if actual == expected_generation => Ok(()),
        Some(Some(actual)) => Err(format!(
            "活动记录 helper generation 不一致: expected={expected_generation}, actual={actual}"
        )),
        Some(None) => Err("活动记录缺少 helper generation".to_string()),
        None => Err("活动记录不存在".to_string()),
    }
}

fn handle_recovery_attempt(attempt: RecoveryAttempt) -> Result<Option<Value>, String> {
    match attempt {
        RecoveryAttempt::Completed(result) | RecoveryAttempt::Streaming(result) => Ok(Some(result)),
        RecoveryAttempt::Stale => Ok(Some(json!({"status": "stale"}))),
        RecoveryAttempt::HelperError(error) => Err(error.into_message()),
        RecoveryAttempt::NotFound => Ok(None),
    }
}

async fn finalize_missing_generation<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    recovery_lease: &CompletionLease,
    msg_id: &str,
) -> Result<Value, String> {
    let active_generation = finalize::active_helper_generation(pool, recovery_lease.key()).await?;
    let Some(active_generation) = active_generation else {
        log::warn!(
            "[VCPClient] 活动记录已不存在，恢复按 stale 处理：msg_id={msg_id}; result=stale"
        );
        return Ok(json!({ "status": "stale" }));
    };
    let Some(generation) = active_generation else {
        finalize::delete_active_generation_if_observed(pool, recovery_lease.key(), None).await?;
        log::warn!(
            "[VCPClient] 活动记录缺少真实 helper generation，已按观察值自愈清理：result=stale"
        );
        return Ok(json!({ "status": "stale" }));
    };
    if finalize::message_has_terminal_state(pool, recovery_lease.key()).await? {
        finalize::delete_active_generation_if_observed(
            pool,
            recovery_lease.key(),
            Some(generation),
        )
        .await?;
        log::info!("[VCPClient] 恢复到达时消息已由正常终结器完成，返回 stale");
        return Ok(json!({ "status": "stale" }));
    }
    log::warn!("[VCPClient] 未找到可恢复的生成，开始写入失败终态：result=error");
    let marked = mark_message_as_error_guarded_with_generation(
        app,
        pool,
        recovery_lease,
        Some("后台进程已被系统销毁，流式对话中断".to_string()),
        Some(generation),
    )
    .await?;
    match marked {
        GuardedTransition::Applied(()) => Ok(json!({
            "status": "failed"
        })),
        GuardedTransition::Skipped => Ok(json!({ "status": "stale" })),
    }
}

#[cfg(test)]
#[path = "vcp_client_recovery_flow_tests.rs"]
mod tests;
