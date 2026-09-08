use super::super::super::transport::{connect_to_helper, send_stop_to_helper};
use super::finalize::RecoveryPayload;
use super::{
    classify_helper_status, CompletionLease, HelperStatus, RecoveryAttempt, RecoveryError,
    RecoveryFinalization,
};
use crate::vcp_modules::chat::topic_types::MessageKey;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Runtime};
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

const HELPER_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn recover_from_helper<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    recovery_lease: &CompletionLease,
) -> Result<RecoveryAttempt, String> {
    log::info!("[VCPClient] 通过 TCP 查询 helper 会话：result=started");
    let response = match query_helper_session(app, key, msg_id).await {
        Ok(response) => response,
        Err(error) => return Ok(super::helper_query_failed(error)),
    };
    let app = app.clone();
    let pool = pool.clone();
    let msg_id = msg_id.to_string();
    let transition = recovery_lease
        .with_current_transition(|current_key| async move {
            handle_helper_response_under_transition(&app, &pool, &current_key, &msg_id, response)
                .await
        })
        .await?;
    Ok(match transition {
        super::GuardedTransition::Applied(attempt) => attempt,
        super::GuardedTransition::Skipped => RecoveryAttempt::Stale,
    })
}

/// 活动请求已由 registry 占用时，只查询 helper 的真实 session generation。
pub(super) async fn query_active_helper_session<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    msg_id: &str,
    expected_generation: u64,
) -> Result<RecoveryAttempt, String> {
    let response = query_helper_session(app, key, msg_id)
        .await
        .map_err(RecoveryError::into_message)?;
    let status = super::validate_helper_status_generation(&response, expected_generation)?;
    match status {
        HelperStatus::Streaming => {
            let content = response["content"].as_str().map(str::to_string);
            let generation = response["generation"].as_u64().filter(|value| *value > 0);
            Ok(handle_streaming_helper_response(
                response, content, generation,
            ))
        }
        HelperStatus::Completed => Ok(RecoveryAttempt::Stale),
        HelperStatus::NotFound => Ok(RecoveryAttempt::NotFound),
    }
}

async fn query_helper_session<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Value, RecoveryError> {
    tokio::time::timeout(
        HELPER_QUERY_TIMEOUT,
        query_helper_session_inner(app, key, msg_id),
    )
    .await
    .map_err(|_| RecoveryError::retryable("helper 查询响应超时，已取消未完成读取"))?
}

async fn query_helper_session_inner<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Value, RecoveryError> {
    let stream = connect_to_helper(app, "query", key, None)
        .await
        .map_err(|error| RecoveryError::retryable(format!("helper 查询连接失败: {error}")))?;
    let mut reader = FramedRead::new(stream, LengthDelimitedCodec::new());
    let frame = reader
        .next()
        .await
        .ok_or_else(|| RecoveryError::retryable("helper 查询连接提前关闭"))?
        .map_err(|error| RecoveryError::retryable(format!("读取 helper 查询响应失败: {error}")))?;
    let response = serde_json::from_slice::<Value>(&frame).map_err(|error| {
        RecoveryError::infrastructure(format!("解析 helper 查询响应失败: {error}"))
    })?;
    super::validate_helper_identity(&response, key, msg_id)?;
    Ok(response)
}

async fn handle_helper_response_under_transition<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    response: Value,
) -> Result<RecoveryAttempt, String> {
    let decoded = match decode_helper_response(&response) {
        Ok(decoded) => decoded,
        Err(error) => return Ok(RecoveryAttempt::HelperError(error)),
    };
    log::info!(
        "[VCPClient] 收到 helper 查询响应：status={:?}, generation={:?}, content_length={}",
        decoded.status,
        decoded.generation,
        decoded.content.as_ref().map_or(0, String::len),
    );
    if decoded.status == HelperStatus::NotFound {
        return Ok(RecoveryAttempt::NotFound);
    }
    if completed_response_is_stale(app, pool, key, msg_id, decoded.status, decoded.generation)
        .await?
    {
        return Ok(RecoveryAttempt::Stale);
    }
    let Some(generation) = decoded.generation.filter(|value| *value > 0) else {
        return Ok(RecoveryAttempt::HelperError(RecoveryError::infrastructure(
            "helper 响应缺少有效 generation",
        )));
    };
    if let Err(attempt) =
        bind_helper_generation_or_report(app, pool, key, msg_id, generation).await?
    {
        return Ok(attempt);
    }
    dispatch_helper_response(app, pool, key, msg_id, response, decoded, generation).await
}

struct DecodedHelperResponse {
    status: HelperStatus,
    content: Option<String>,
    finish_reason: Option<String>,
    generation: Option<u64>,
}

fn decode_helper_response(response: &Value) -> Result<DecodedHelperResponse, RecoveryError> {
    Ok(DecodedHelperResponse {
        status: classify_helper_status(response)?,
        content: response["content"].as_str().map(str::to_string),
        finish_reason: response["lastFinishReason"].as_str().map(str::to_string),
        generation: response["generation"].as_u64(),
    })
}

async fn completed_response_is_stale<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    status: HelperStatus,
    generation: Option<u64>,
) -> Result<bool, String> {
    if status != HelperStatus::Completed
        || super::finalize::active_helper_generation(pool, key)
            .await?
            .is_some()
        || !super::finalize::message_has_terminal_state(pool, key).await?
    {
        return Ok(false);
    }
    if let Some(generation) = generation.filter(|value| *value > 0) {
        best_effort_stop_helper(app, msg_id, key, generation).await;
    }
    log::info!("[VCPClient] 活动行已缺失且目标消息已有终态，恢复按 stale 幂等完成：result=stale");
    Ok(true)
}

async fn bind_helper_generation_or_report<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    generation: u64,
) -> Result<Result<(), RecoveryAttempt>, String> {
    if super::finalize::bind_active_helper_generation(pool, key, generation).await? {
        return Ok(Ok(()));
    }
    let stop_error = match send_stop_to_helper(app, msg_id, key, generation).await {
        Ok(()) => String::new(),
        Err(error) => format!("；精确停止旧 helper generation 失败: {error}"),
    };
    Ok(Err(RecoveryAttempt::HelperError(
        RecoveryError::infrastructure(format!(
            "helper generation 与活动记录不一致，已请求精确停止{stop_error}"
        )),
    )))
}

async fn dispatch_helper_response<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    response: Value,
    decoded: DecodedHelperResponse,
    generation: u64,
) -> Result<RecoveryAttempt, String> {
    match decoded.status {
        HelperStatus::Completed => {
            handle_completed_helper_response(
                app,
                pool,
                key,
                msg_id,
                decoded.content,
                decoded.finish_reason,
                generation,
            )
            .await
        }
        HelperStatus::Streaming => Ok(handle_streaming_helper_response(
            response,
            decoded.content,
            Some(generation),
        )),
        HelperStatus::NotFound => Ok(RecoveryAttempt::NotFound),
    }
}

async fn handle_completed_helper_response<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    content: Option<String>,
    finish_reason: Option<String>,
    generation: u64,
) -> Result<RecoveryAttempt, String> {
    let Some(content) = content else {
        return Ok(RecoveryAttempt::HelperError(RecoveryError::infrastructure(
            "helper completed 响应缺少 content",
        )));
    };
    if generation == 0 {
        return Ok(RecoveryAttempt::HelperError(RecoveryError::infrastructure(
            "helper completed 响应的 generation 必须为正整数",
        )));
    }
    let payload = RecoveryPayload {
        content: content.clone(),
        finish_reason: finish_reason.or(Some("completed".to_string())),
        generation,
    };
    match super::finalize::finalize_if_active_under_transition(app, pool, key, &payload).await? {
        RecoveryFinalization::Applied => {}
        RecoveryFinalization::Skipped => return Ok(RecoveryAttempt::Stale),
        RecoveryFinalization::NotFound => return Ok(RecoveryAttempt::NotFound),
    }
    best_effort_stop_helper(app, msg_id, key, generation).await;
    Ok(RecoveryAttempt::Completed(json!({
        "status": "completed",
        "content": content,
        "helperGeneration": generation
    })))
}

fn handle_streaming_helper_response(
    response: Value,
    content: Option<String>,
    generation: Option<u64>,
) -> RecoveryAttempt {
    let Some(generation) = generation.filter(|value| *value > 0) else {
        return RecoveryAttempt::HelperError(RecoveryError::infrastructure(
            "helper streaming 响应缺少 generation",
        ));
    };
    RecoveryAttempt::Streaming(json!({
        "status": "streaming",
        "content": content.unwrap_or_default(),
        "lastEventIndex": response["lastEventIndex"],
        "helperGeneration": generation
    }))
}

/// 数据库终态已提交后，helper stop 只是可重试的清理工作。
/// 失败不能回滚或伪报已提交的 completed 结果；仅保留带完整身份与
/// generation 的可观测清理债务日志，供后续重试。
async fn best_effort_stop_helper<R: Runtime>(
    app: &AppHandle<R>,
    msg_id: &str,
    key: &MessageKey,
    generation: u64,
) {
    if send_stop_to_helper(app, msg_id, key, generation)
        .await
        .is_err()
    {
        log::warn!(
            "[VCPClient] helper completed 已提交，stop 清理债务待重试：generation={generation}, result=retry"
        );
    }
}
