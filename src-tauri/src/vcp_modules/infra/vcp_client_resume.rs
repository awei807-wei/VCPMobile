use super::{
    db_pool_if_ready, handle_streaming_request, mark_message_as_error_guarded_with_channel,
    ActiveRequestGuard, ActiveRequests, CompletionLease, GuardedTransition, StreamEvent,
};
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, Runtime};
use tokio::sync::oneshot;

#[path = "vcp_client_resume_preparation.rs"]
mod preparation;
#[cfg(target_os = "android")]
use preparation::cancel_prepared_resume;
use preparation::prepare_resume_request;

#[tauri::command]
#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
pub async fn resume_stream<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, ActiveRequests>,
    msg_id: String,
    topic_id: String,
    owner_id: String,
    owner_type: String,
    stream_channel: Channel<StreamEvent>,
    initial_content: Option<String>,
    last_event_index: Option<i64>,
    expected_generation: Option<u64>,
) -> Result<Value, String> {
    let expected_generation = require_expected_generation(expected_generation)?;
    let request_key =
        super::registry::message_key_from_parts(&owner_id, &owner_type, &topic_id, &msg_id)?;
    log::info!(
        "[VCPClient] resume_stream called for messageId: {}, topicId: {}, lastEventIndex: {:?}",
        msg_id,
        topic_id,
        last_event_index
    );

    let Some(prepared) = prepare_resume_request(
        &app,
        &state,
        &request_key,
        &msg_id,
        &owner_id,
        &owner_type,
        &topic_id,
        initial_content.as_deref(),
        &stream_channel,
        expected_generation,
    )
    .await?
    else {
        return Ok(json!({"status": "skipped", "finalization": "skipped"}));
    };
    let result = run_prepared_resume_request(
        &app,
        &state,
        prepared,
        msg_id.clone(),
        request_key.clone(),
        stream_channel.clone(),
        last_event_index,
        initial_content.clone(),
        expected_generation,
    )
    .await;
    #[cfg(target_os = "android")]
    if let Err(error) = &result {
        cancel_prepared_resume(&app, &request_key, expected_generation, error).await;
    }
    result
}

pub(super) struct PreparedResumeRequest {
    pub(super) pool: sqlx::Pool<sqlx::Sqlite>,
    pub(super) abort_rx: oneshot::Receiver<()>,
    pub(super) request_epoch: u64,
    pub(super) _guard: ActiveRequestGuard,
    pub(super) completion_lease: CompletionLease,
    pub(super) client: reqwest::Client,
    pub(super) context: Value,
}

async fn run_prepared_resume_request<R: Runtime>(
    app: &AppHandle<R>,
    state: &tauri::State<'_, ActiveRequests>,
    prepared: PreparedResumeRequest,
    msg_id: String,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
    stream_channel: Channel<StreamEvent>,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
    expected_generation: u64,
) -> Result<Value, String> {
    let PreparedResumeRequest {
        pool,
        abort_rx,
        request_epoch,
        _guard,
        completion_lease,
        client,
        context,
    } = prepared;
    let (mut response, is_aborted) = execute_resume_request(
        app,
        &pool,
        client,
        msg_id,
        request_key,
        request_epoch,
        context,
        abort_rx,
        state.0.clone(),
        stream_channel.clone(),
        last_event_index,
        initial_content,
        completion_lease.clone(),
        expected_generation,
    )
    .await?;
    let finalization = finalize_resumed_message(
        app,
        &pool,
        &response,
        is_aborted,
        stream_channel,
        &completion_lease,
    )
    .await?;
    if matches!(
        finalization,
        crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped
    ) {
        response["finalization"] = json!("skipped");
    }
    drop(completion_lease);
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
async fn execute_resume_request<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    client: reqwest::Client,
    msg_id: String,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
    request_epoch: u64,
    context: Value,
    abort_rx: oneshot::Receiver<()>,
    active_requests: std::sync::Arc<super::ActiveRequestRegistry>,
    stream_channel: Channel<StreamEvent>,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
    completion_lease: CompletionLease,
    expected_generation: u64,
) -> Result<(Value, bool), String> {
    let stream_channel_for_error = stream_channel.clone();
    match run_resume_request(
        app,
        client,
        msg_id,
        request_key.clone(),
        request_epoch,
        context,
        abort_rx,
        active_requests,
        stream_channel,
        last_event_index,
        initial_content,
        expected_generation,
    )
    .await
    {
        Ok(value) => Ok(value),
        Err(error) => {
            log::error!(
                "[VCPClient] resume_stream failed during handle_streaming_request: {}",
                error
            );
            let mark_result = mark_message_as_error_guarded_with_channel(
                app,
                pool,
                &completion_lease,
                Some(format!("接续失败: {}", error)),
                Some(stream_channel_for_error),
            )
            .await;
            match mark_result {
                Ok(GuardedTransition::Applied(())) => {}
                Ok(GuardedTransition::Skipped) => {
                    log::warn!("[VCPClient] 接续失败终结已跳过旧请求");
                }
                Err(mark_error) => {
                    return Err(format!("{error}; 错误消息持久化失败: {mark_error}"));
                }
            }
            Err(error)
        }
    }
}

async fn persist_initial_content<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    content: Option<&str>,
    lease: &CompletionLease,
) -> Result<crate::vcp_modules::chat::message_service::StreamFinalizationStatus, String> {
    let pool = pool.clone();
    let content = content.map(str::to_string);
    let transition = lease
        .with_current_transition(|key| async move {
            let Some(content) = content else {
                return Ok(());
            };
            crate::vcp_modules::chat::message_service::update_existing_message_content(
                app_handle.clone(),
                &pool,
                &key,
                content,
                None,
                true,
            )
            .await
            .map(|_| ())
        })
        .await?;
    Ok(match transition {
        GuardedTransition::Applied(()) => {
            crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Applied
        }
        GuardedTransition::Skipped => {
            crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped
        }
    })
}

fn require_expected_generation(expected_generation: Option<u64>) -> Result<u64, String> {
    expected_generation
        .filter(|generation| *generation > 0)
        .ok_or_else(|| "resume_stream 缺少有效的正整数 helper generation".to_string())
}

#[allow(clippy::too_many_arguments)]
async fn run_resume_request<R: Runtime>(
    app: &AppHandle<R>,
    client: reqwest::Client,
    msg_id: String,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
    request_epoch: u64,
    context: Value,
    abort_rx: oneshot::Receiver<()>,
    active_requests: std::sync::Arc<super::ActiveRequestRegistry>,
    stream_channel: Channel<StreamEvent>,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
    expected_generation: u64,
) -> Result<(Value, bool), String> {
    handle_streaming_request(
        app,
        client,
        "",
        "",
        Value::Null,
        msg_id,
        request_key,
        request_epoch,
        Some(context),
        abort_rx,
        active_requests,
        Some(stream_channel),
        true,
        last_event_index,
        initial_content,
        Some(expected_generation),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finalize_resumed_message<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    response: &Value,
    is_aborted: bool,
    stream_channel: Channel<StreamEvent>,
    completion_lease: &CompletionLease,
) -> Result<crate::vcp_modules::chat::message_service::StreamFinalizationStatus, String> {
    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        response["finishReason"].as_str().map(str::to_string)
    };
    log::info!("[VCPClient] resume_stream completed. Finalizing message.");
    let Some(full_content) = response["fullContent"].as_str() else {
        let mark_result = mark_message_as_error_guarded_with_channel(
            app,
            pool,
            completion_lease,
            Some("接续响应缺少 fullContent".to_string()),
            Some(stream_channel),
        )
        .await?;
        return match mark_result {
            GuardedTransition::Applied(()) => Err("接续响应缺少 fullContent".to_string()),
            GuardedTransition::Skipped => {
                Ok(crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped)
            }
        };
    };
    crate::vcp_modules::chat::message_service::finalize_stream_message_guarded(
        app.clone(),
        pool,
        completion_lease,
        full_content.to_string(),
        is_aborted,
        finish_reason,
        Some(stream_channel),
        (completion_lease.key().topic.owner_type == "agent")
            .then(|| completion_lease.key().topic.owner_id.clone()),
    )
    .await
}

#[cfg(test)]
#[path = "vcp_client_resume_tests.rs"]
mod tests;
