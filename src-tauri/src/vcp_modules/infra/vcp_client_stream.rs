use super::{ActiveRequestRegistry, MessageKey, StreamEvent};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, Runtime};

#[path = "vcp_client_stream/context.rs"]
mod context;

/// Drive one streaming request through connection, recovery, parsing and finalization.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_streaming_request<R: Runtime>(
    app: &AppHandle<R>,
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: Value,
    message_id: String,
    request_key: MessageKey,
    request_epoch: u64,
    context: Option<Value>,
    abort_rx: tokio::sync::oneshot::Receiver<()>,
    active_requests: Arc<ActiveRequestRegistry>,
    stream_channel: Option<Channel<StreamEvent>>,
    is_resume: bool,
    last_event_index: Option<i64>,
    initial_content: Option<String>,
    helper_generation: Option<u64>,
) -> Result<(Value, bool), String> {
    context::run_streaming_request(context::StreamRequest {
        app: app.clone(),
        client,
        final_url: final_url.to_string(),
        api_key: api_key.to_string(),
        request_body,
        message_id,
        request_key,
        request_epoch,
        context,
        abort_rx,
        active_requests,
        stream_channel,
        is_resume,
        last_event_index,
        initial_content,
        helper_generation,
    })
    .await
}
