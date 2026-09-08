use super::{
    handle_non_streaming_request, handle_streaming_request, load_app_settings,
    message_key_from_context, ActiveRequestGuard, ActiveRequestRegistry, CompletionLease,
    StreamEvent, VcpRequestPayload,
};
use crate::vcp_modules::infra::utils::normalize_vcp_url;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::{AppHandle, Runtime};
use tokio::sync::oneshot;
use url::Url;

#[path = "vcp_client_preprocess_messages.rs"]
mod messages;
pub(super) use messages::preprocess_multimodal_messages;

#[path = "vcp_client_preprocess_timestamp.rs"]
mod timestamp;
use timestamp::extract_timestamp_bindings;

/// 请求执行结果。网络层完成后仍持有消息身份租约，交由调用方在持久化后释放。
pub struct VcpRequestOutcome {
    pub response: Value,
    pub is_aborted: bool,
    pub completion_lease: CompletionLease,
    /// The request guard must outlive the network dispatch and the caller's
    /// guarded finalizer.  Dropping it inside `dispatch_vcp_request` removes
    /// the current epoch before persistence can run.
    pub(crate) request_guard: ActiveRequestGuard,
}

/// 请求失败时保留租约，调用方可以用同一身份安全清理错误状态。
pub struct VcpRequestError {
    pub message: String,
    pub completion_lease: Option<CompletionLease>,
    pub stale: bool,
    /// Kept until the caller has finished success/error/cancel finalization.
    pub(crate) request_guard: Option<ActiveRequestGuard>,
}

impl VcpRequestError {
    fn without_lease(message: String) -> Self {
        Self {
            message,
            completion_lease: None,
            stale: false,
            request_guard: None,
        }
    }

    fn with_lease(
        message: String,
        completion_lease: CompletionLease,
        request_guard: ActiveRequestGuard,
    ) -> Self {
        Self {
            message,
            completion_lease: Some(completion_lease),
            stale: false,
            request_guard: Some(request_guard),
        }
    }

    fn skipped() -> Self {
        Self {
            message: "请求已被新请求替换".to_string(),
            completion_lease: None,
            stale: true,
            request_guard: None,
        }
    }
}

enum PrepareError {
    Failed(String),
    Skipped,
}

impl From<String> for PrepareError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// 核心请求实现函数，可供 Tauri Command 或内部 Rust 模块调用。
pub async fn perform_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<ActiveRequestRegistry>,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<VcpRequestOutcome, VcpRequestError> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );
    let prepared =
        match prepare_vcp_request(app, active_requests, payload, stream_channel.as_ref()).await {
            Ok(prepared) => prepared,
            Err(PrepareError::Skipped) => return Err(VcpRequestError::skipped()),
            Err(PrepareError::Failed(error)) => return Err(VcpRequestError::without_lease(error)),
        };
    let completion_lease = prepared.completion_lease.clone();
    match dispatch_vcp_request(prepared, stream_channel).await {
        Ok(((response, is_aborted), request_guard)) => Ok(VcpRequestOutcome {
            response,
            is_aborted,
            completion_lease,
            request_guard,
        }),
        Err((error, request_guard)) => Err(VcpRequestError::with_lease(
            error,
            completion_lease,
            request_guard,
        )),
    }
}

struct PreparedRequest<R: Runtime> {
    app: AppHandle<R>,
    client: Client,
    final_url: String,
    api_key: String,
    request_body: Value,
    message_id: String,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
    request_epoch: u64,
    context: Option<Value>,
    abort_rx: oneshot::Receiver<()>,
    active_requests: Arc<ActiveRequestRegistry>,
    _guard: ActiveRequestGuard,
    completion_lease: CompletionLease,
    is_stream: bool,
}

async fn prepare_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<ActiveRequestRegistry>,
    payload: VcpRequestPayload,
    stream_channel: Option<&Channel<StreamEvent>>,
) -> Result<PreparedRequest<R>, PrepareError> {
    let message_id = payload.message_id.clone();
    let context = payload.context.clone();
    let request_key = message_key_from_context(context.as_ref(), &message_id)?;
    let messages = preprocess_multimodal_messages(app, payload.messages).await?;
    let final_url = resolve_request_url(app, &payload.vcp_url).await;
    let mut messages = messages;
    ensure_system_message(&mut messages);
    let timestamp_bindings = extract_timestamp_bindings(&mut messages);
    let is_stream = payload.model_config["stream"].as_bool().unwrap_or(false);
    let request_body = build_request_body(
        &payload.model_config,
        messages,
        &message_id,
        is_stream,
        timestamp_bindings,
    );
    let client = Client::builder()
        .tcp_keepalive(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let (abort_rx, request_epoch, completion_lease) =
        register_request(&active_requests, request_key.clone()).await;
    let guard =
        ActiveRequestGuard::new(active_requests.clone(), request_key.clone(), request_epoch);
    if is_stream {
        prepare_stream_request(
            app,
            context.as_ref(),
            &completion_lease,
            stream_channel,
            &message_id,
            request_epoch,
        )
        .await?;
    }
    Ok(PreparedRequest {
        app: app.clone(),
        client,
        final_url,
        api_key: payload.vcp_api_key,
        request_body,
        message_id,
        request_key,
        request_epoch,
        context,
        abort_rx,
        active_requests,
        _guard: guard,
        completion_lease,
        is_stream,
    })
}

async fn prepare_stream_request<R: Runtime>(
    app: &AppHandle<R>,
    context: Option<&Value>,
    completion_lease: &CompletionLease,
    stream_channel: Option<&Channel<StreamEvent>>,
    message_id: &str,
    request_epoch: u64,
) -> Result<(), PrepareError> {
    let pool = super::db_pool_if_ready(app)?;
    let agent_id = context
        .and_then(|value| {
            value["agentId"]
                .as_str()
                .or_else(|| value["speakerAgentId"].as_str())
        })
        .map(str::to_string);
    let name = context
        .and_then(|value| value["agentName"].as_str())
        .map(str::to_string);
    let status = crate::vcp_modules::chat::message_service::persist_stream_skeleton_guarded(
        app.clone(),
        &pool,
        completion_lease,
        agent_id,
        name,
    )
    .await?;
    if matches!(
        status,
        crate::vcp_modules::chat::message_service::StreamFinalizationStatus::Skipped
    ) {
        return Err(PrepareError::Skipped);
    }
    send_thinking_event(stream_channel, message_id, context, request_epoch)
}

fn send_thinking_event(
    stream_channel: Option<&Channel<StreamEvent>>,
    message_id: &str,
    context: Option<&Value>,
    generation: u64,
) -> Result<(), PrepareError> {
    let Some(channel) = stream_channel else {
        return Ok(());
    };
    channel
        .send(StreamEvent::thinking(
            message_id.to_string(),
            context.cloned(),
            generation,
        ))
        .map_err(|error| PrepareError::Failed(format!("发送流式 thinking 事件失败: {error}")))
}

async fn resolve_request_url<R: Runtime>(app: &AppHandle<R>, raw_url: &str) -> String {
    let enable_tool_injection = load_app_settings(app)
        .await
        .ok()
        .and_then(|settings| {
            settings
                .extra
                .as_object()
                .and_then(|extra| extra.get("enableVcpToolInjection"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    if enable_tool_injection {
        if let Ok(mut url) = Url::parse(raw_url) {
            url.set_path("/v1/chatvcp/completions");
            return url.to_string();
        }
    }
    normalize_vcp_url(raw_url)
}

fn ensure_system_message(messages: &mut Vec<Value>) {
    if !messages.iter().any(|message| message["role"] == "system") {
        messages.insert(0, json!({"role": "system", "content": ""}));
    }
}

fn build_request_body(
    model_config: &Value,
    messages: Vec<Value>,
    message_id: &str,
    is_stream: bool,
    timestamp_bindings: Vec<Value>,
) -> Value {
    let mut request_body = model_config.clone();
    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("messages".to_string(), json!(messages));
        obj.insert("requestId".to_string(), json!(message_id));
        obj.insert("stream".to_string(), json!(is_stream));
        if !timestamp_bindings.is_empty() {
            obj.insert(
                "vcpchatExtensions".to_string(),
                json!({
                    "schemaVersion": 1,
                    "messageMetadataMode": "hash_only",
                    "messageTimestampBindings": timestamp_bindings
                }),
            );
        }
    }
    request_body
}

async fn register_request(
    active_requests: &ActiveRequestRegistry,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
) -> (oneshot::Receiver<()>, u64, CompletionLease) {
    let (abort_tx, abort_rx) = oneshot::channel();
    let (request_epoch, previous_sender, completion_lease) =
        active_requests.register(request_key, abort_tx).await;
    if let Some(previous_sender) = previous_sender {
        let _ = previous_sender.send(());
    }
    (abort_rx, request_epoch, completion_lease)
}

async fn dispatch_vcp_request<R: Runtime>(
    prepared: PreparedRequest<R>,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<((Value, bool), ActiveRequestGuard), (String, ActiveRequestGuard)> {
    let (result, request_guard) = dispatch_request_payload(prepared, stream_channel).await;
    match result {
        Ok(value) => Ok((value, request_guard)),
        Err(error) => Err((error, request_guard)),
    }
}

async fn dispatch_request_payload<R: Runtime>(
    prepared: PreparedRequest<R>,
    stream_channel: Option<Channel<StreamEvent>>,
) -> (Result<(Value, bool), String>, ActiveRequestGuard) {
    let PreparedRequest {
        app,
        client,
        final_url,
        api_key,
        request_body,
        message_id,
        request_key,
        request_epoch,
        context,
        abort_rx,
        active_requests,
        _guard: request_guard,
        completion_lease: _,
        is_stream,
    } = prepared;

    // === 7. 分发至专职处理器执行请求 ===
    let result = if is_stream {
        handle_streaming_request(
            &app,
            client,
            &final_url,
            &api_key,
            request_body,
            message_id,
            request_key,
            request_epoch,
            context,
            abort_rx,
            active_requests,
            stream_channel,
            false,
            None,
            None,
            None,
        )
        .await
    } else {
        handle_non_streaming_request(
            client,
            &final_url,
            &api_key,
            request_body,
            message_id,
            request_key,
            request_epoch,
            context,
            abort_rx,
            active_requests,
            stream_channel,
        )
        .await
    };
    (result, request_guard)
}
