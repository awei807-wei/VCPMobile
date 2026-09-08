use super::{ActiveRequestRegistry, AuroraUpdate, MessageKey, StreamEvent};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::ipc::Channel;

/// 4. 抽离非流式请求循环
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_non_streaming_request(
    client: Client,
    final_url: &str,
    api_key: &str,
    request_body: Value,
    message_id: String,
    request_key: MessageKey,
    request_epoch: u64,
    context: Option<Value>,
    mut abort_rx: tokio::sync::oneshot::Receiver<()>,
    active_requests: Arc<ActiveRequestRegistry>,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    let response = match request_non_stream_response(
        &client,
        final_url,
        api_key,
        &request_body,
        &message_id,
        &request_key,
        request_epoch,
        &context,
        &mut abort_rx,
        &active_requests,
        &stream_channel,
    )
    .await
    {
        Ok(response) => response,
        Err(NonStreamFailure::Cancelled(result)) => return Ok((result, true)),
        Err(NonStreamFailure::Error(error)) => return Err(error),
    };
    active_requests.remove_entry_if_current(&request_key, request_epoch);
    parse_non_stream_response(
        response,
        &message_id,
        request_epoch,
        context,
        &stream_channel,
    )
    .await
}

enum NonStreamFailure {
    Cancelled(Value),
    Error(String),
}

#[allow(clippy::too_many_arguments)]
async fn request_non_stream_response(
    client: &Client,
    final_url: &str,
    api_key: &str,
    request_body: &Value,
    message_id: &str,
    request_key: &MessageKey,
    request_epoch: u64,
    context: &Option<Value>,
    abort_rx: &mut tokio::sync::oneshot::Receiver<()>,
    active_requests: &Arc<ActiveRequestRegistry>,
    stream_channel: &Option<Channel<StreamEvent>>,
) -> Result<reqwest::Response, NonStreamFailure> {
    let request_future = client
        .post(final_url)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .header(CONTENT_TYPE, "application/json")
        .json(request_body)
        .send();
    tokio::select! {
        _ = abort_rx => {
            log::warn!("[VCPClient] Non-streaming request aborted before response for message: {message_id}");
            send_stream_event(stream_channel, StreamEvent::error(
                message_id.to_string(), context.clone(), "请求已中止".to_string(), request_epoch,
            ));
            active_requests.remove_entry_if_current(request_key, request_epoch);
            Err(NonStreamFailure::Cancelled(json!({
                "response": Value::Null,
                "fullContent": "",
                "finishReason": "cancelled_by_user",
                "context": context
            })))
        }
        result = request_future => match result {
            Ok(response) => Ok(response),
            Err(error) => {
                let error = format!("VCP请求失败: {error}");
                send_stream_event(stream_channel, StreamEvent::error(
                    message_id.to_string(), context.clone(), error.clone(), request_epoch,
                ));
                active_requests.remove_entry_if_current(request_key, request_epoch);
                Err(NonStreamFailure::Error(error))
            }
        }
    }
}

async fn parse_non_stream_response(
    response: reqwest::Response,
    message_id: &str,
    request_epoch: u64,
    context: Option<Value>,
    stream_channel: &Option<Channel<StreamEvent>>,
) -> Result<(Value, bool), String> {
    let status = response.status();
    if !status.is_success() {
        return Err(emit_non_stream_http_error(
            response,
            message_id,
            request_epoch,
            &context,
            stream_channel,
        )
        .await);
    }
    let vcp_response = decode_non_stream_json(
        response,
        message_id,
        request_epoch,
        &context,
        stream_channel,
    )
    .await?;
    Ok(build_non_stream_success(
        vcp_response,
        message_id,
        request_epoch,
        context,
        stream_channel,
    ))
}

async fn emit_non_stream_http_error(
    response: reqwest::Response,
    message_id: &str,
    request_epoch: u64,
    context: &Option<Value>,
    stream_channel: &Option<Channel<StreamEvent>>,
) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let error = format!("VCP服务器错误: {status} - {text}");
    send_stream_event(
        stream_channel,
        StreamEvent::error(
            message_id.to_string(),
            context.clone(),
            error.clone(),
            request_epoch,
        ),
    );
    error
}

async fn decode_non_stream_json(
    response: reqwest::Response,
    message_id: &str,
    request_epoch: u64,
    context: &Option<Value>,
    stream_channel: &Option<Channel<StreamEvent>>,
) -> Result<Value, String> {
    match response.json::<Value>().await {
        Ok(value) => Ok(value),
        Err(error) => {
            let error = format!("JSON解析失败: {error}");
            send_stream_event(
                stream_channel,
                StreamEvent::error(
                    message_id.to_string(),
                    context.clone(),
                    error.clone(),
                    request_epoch,
                ),
            );
            Err(error)
        }
    }
}

fn build_non_stream_success(
    vcp_response: Value,
    message_id: &str,
    request_epoch: u64,
    context: Option<Value>,
    stream_channel: &Option<Channel<StreamEvent>>,
) -> (Value, bool) {
    let (full_content, finish_reason) = extract_non_stream_content(&vcp_response);
    send_stream_event(
        stream_channel,
        StreamEvent::aurora(
            message_id.to_string(),
            AuroraUpdate {
                stable_blocks: None,
                stable_changed: false,
                tail_block: None,
                tail: None,
                tail_changed: false,
                tail_frame: None,
                tail_snapshot: None,
                content: Some(full_content.clone()),
                chunk: None,
            },
            context.clone(),
            request_epoch,
        ),
    );
    (
        json!({
            "response": vcp_response,
            "fullContent": full_content,
            "finishReason": finish_reason,
            "context": context
        }),
        false,
    )
}

fn extract_non_stream_content(response: &Value) -> (String, Option<String>) {
    let first_choice = response["choices"]
        .as_array()
        .and_then(|choices| choices.first());
    let content = first_choice
        .and_then(|choice| choice["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
    let finish_reason = first_choice
        .and_then(|choice| choice["finish_reason"].as_str())
        .map(|reason| {
            if reason == "stop" {
                "completed".to_string()
            } else {
                reason.to_string()
            }
        });
    (content, finish_reason)
}

fn send_stream_event(channel: &Option<Channel<StreamEvent>>, event: StreamEvent) {
    if let Some(channel) = channel {
        let _ = channel.send(event);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 非流式响应只取首项并将stop映射为completed() {
        let response = serde_json::json!({
            "choices": [
                {"message": {"content": "首项"}, "finish_reason": "stop"},
                {"message": {"content": "后项"}, "finish_reason": "length"}
            ]
        });
        assert_eq!(
            super::extract_non_stream_content(&response),
            ("首项".to_string(), Some("completed".to_string()))
        );
    }
}
