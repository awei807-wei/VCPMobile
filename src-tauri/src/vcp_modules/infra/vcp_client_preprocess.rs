use super::{
    handle_non_streaming_request, handle_streaming_request, load_app_settings,
    message_key_from_context, ActiveRequestGuard, ActiveRequestRegistry, StreamEvent,
    VcpRequestPayload,
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

fn extract_text_for_hash(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let text_parts: Vec<String> = arr
            .iter()
            .filter(|part| part["type"].as_str() == Some("text"))
            .filter_map(|part| part["text"].as_str())
            .map(|s| s.to_string())
            .collect();
        return text_parts.join("\n");
    }
    if let Some(obj) = content.as_object() {
        if let Some(s) = obj.get("text").and_then(|t| t.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

fn get_or_calculate_message_hash(content: &Value) -> String {
    use crate::vcp_modules::infra::utils::calculate_sha256;

    let text = extract_text_for_hash(content);
    let hash = calculate_sha256(text.as_bytes());
    format!("sha256:{}", hash)
}

/// 核心请求实现函数，可供 Tauri Command 或 内部 Rust 模块(如 GroupOrchestrator) 调用
/// 返回 Result<(全量内容/响应体, 是否被中止), 错误信息>
pub async fn perform_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<ActiveRequestRegistry>,
    payload: VcpRequestPayload,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    log::info!(
        "[VCPClient] perform_vcp_request called for messageId: {}, context: {:?}",
        payload.message_id,
        payload.context
    );
    let prepared = prepare_vcp_request(app, active_requests, payload).await?;
    dispatch_vcp_request(prepared, stream_channel).await
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
    is_stream: bool,
}

async fn prepare_vcp_request<R: Runtime>(
    app: &AppHandle<R>,
    active_requests: Arc<ActiveRequestRegistry>,
    payload: VcpRequestPayload,
) -> Result<PreparedRequest<R>, String> {
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
    let (abort_rx, request_epoch, previous_sender) =
        register_request(&active_requests, request_key.clone());
    if let Some(previous_sender) = previous_sender {
        let _ = previous_sender.send(());
    }
    let guard =
        ActiveRequestGuard::new(active_requests.clone(), request_key.clone(), request_epoch);
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
        is_stream,
    })
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

fn register_request(
    active_requests: &ActiveRequestRegistry,
    request_key: crate::vcp_modules::chat::topic_types::MessageKey,
) -> (oneshot::Receiver<()>, u64, Option<oneshot::Sender<()>>) {
    let (abort_tx, abort_rx) = oneshot::channel();
    let (request_epoch, previous_sender) = active_requests.register(request_key, abort_tx);
    (abort_rx, request_epoch, previous_sender)
}

async fn dispatch_vcp_request<R: Runtime>(
    prepared: PreparedRequest<R>,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
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
        _guard,
        is_stream,
    } = prepared;

    // === 7. 分发至专职处理器执行请求 ===
    if is_stream {
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
    }
}

/// 2. 抽离时间戳与哈希绑定生成逻辑
pub(super) fn extract_timestamp_bindings(messages: &mut [Value]) -> Vec<Value> {
    let mut message_timestamp_bindings = Vec::new();
    for (index, msg) in messages.iter_mut().enumerate() {
        let mut timestamp_meta = None;
        if let Some(obj) = msg.as_object_mut() {
            if let Some(meta) = obj.remove("__vcpchatTimestampMeta") {
                timestamp_meta = Some(meta);
            }
        }
        if let Some(meta) = timestamp_meta {
            if let (Some(message_id), Some(role), Some(timestamp)) = (
                meta.get("messageId").and_then(|id| id.as_str()),
                meta.get("role").and_then(|r| r.as_str()),
                meta.get("timestamp").and_then(|t| t.as_u64()),
            ) {
                use chrono::TimeZone;
                let timestamp_iso =
                    if let Some(dt) = chrono::Utc.timestamp_millis_opt(timestamp as i64).single() {
                        dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    } else {
                        "".to_string()
                    };

                let final_content_val = msg.get("content").unwrap_or(&Value::Null);
                let sent_message_hash = get_or_calculate_message_hash(final_content_val);

                message_timestamp_bindings.push(json!({
                    "messageId": message_id,
                    "role": role,
                    "timestamp": timestamp,
                    "timestampIso": timestamp_iso,
                    "source": "client_history",
                    "sentMessageHash": sent_message_hash,
                    "sentMessageIndex": index
                }));
            }
        }
    }
    message_timestamp_bindings
}
