use super::{State, StreamControl, StreamSession, StreamSource};
#[cfg(not(target_os = "android"))]
use futures_util::TryStreamExt;
#[cfg(not(target_os = "android"))]
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
#[cfg(target_os = "android")]
use serde_json::json;
#[cfg(target_os = "android")]
use serde_json::Value;
use std::time::Duration;
use tauri::Runtime;
#[cfg(not(target_os = "android"))]
use tokio_util::io::StreamReader;

#[cfg(target_os = "android")]
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};
#[cfg(not(target_os = "android"))]
use tokio_util::codec::{FramedRead, LinesCodec};

pub(super) async fn connect<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    #[cfg(target_os = "android")]
    {
        connect_android(session).await
    }
    #[cfg(not(target_os = "android"))]
    {
        connect_desktop(session).await
    }
}

#[cfg(target_os = "android")]
async fn connect_android<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let headers = json!({
        "Authorization": format!("Bearer {}", session.api_key),
        "Content-Type": "application/json"
    });
    let params = json!({
        "url": session.final_url,
        "headers": headers.to_string(),
        "body": session.request_body.to_string(),
        "context": build_sse_context(session)
    });
    match session.connect_helper("start", Some(params)).await {
        Ok(stream) => {
            session.source = Some(StreamSource::Tcp(FramedRead::new(
                stream,
                LengthDelimitedCodec::new(),
            )));
            StreamControl::Continue(State::Streaming)
        }
        Err(error) => {
            log::error!("[VCPClient] connect_to_helper failed: {:?}", error);
            session.fail_with_error(format!("启动本地代理失败: {}", error))
        }
    }
}

#[cfg(target_os = "android")]
fn build_sse_context<R: Runtime>(session: &StreamSession<R>) -> Value {
    let mut context = json!({});
    if let Some(agent_name) = session
        .context
        .as_ref()
        .and_then(|value| value.get("agentName"))
        .and_then(Value::as_str)
    {
        context["agentName"] = json!(agent_name);
    }
    if let Some(topic_id) = session
        .context
        .as_ref()
        .and_then(|value| value.get("topicId"))
        .and_then(Value::as_str)
    {
        context["topicId"] = json!(topic_id);
    }
    let owner_id = session
        .context
        .as_ref()
        .and_then(|value| value.get("groupId"))
        .and_then(Value::as_str)
        .or_else(|| {
            session
                .context
                .as_ref()
                .and_then(|value| value.get("agentId"))
                .and_then(Value::as_str)
        });
    if let Some(owner_id) = owner_id {
        context["ownerId"] = json!(owner_id);
    }
    let owner_type = session
        .context
        .as_ref()
        .and_then(|value| value.get("ownerType"))
        .and_then(Value::as_str)
        .or_else(|| {
            session
                .context
                .as_ref()
                .and_then(|value| value.get("groupId"))
                .and_then(Value::as_str)
                .map(|_| "group")
        })
        .or_else(|| {
            session
                .context
                .as_ref()
                .and_then(|value| value.get("agentId"))
                .and_then(Value::as_str)
                .map(|_| "agent")
        });
    if let Some(owner_type) = owner_type {
        context["ownerType"] = json!(owner_type);
    }
    context
}

#[cfg(not(target_os = "android"))]
async fn connect_desktop<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let request = session
        .client
        .post(&session.final_url)
        .header(AUTHORIZATION, format!("Bearer {}", session.api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&session.request_body)
        .send();
    let response = tokio::select! {
        _ = &mut session.abort_rx => return session.cancel_before_streaming(),
        result = request => result,
    };
    match response {
        Ok(response) if response.status().is_success() => {
            session.source = Some(StreamSource::Lines(to_line_stream(response)));
            StreamControl::Continue(State::Streaming)
        }
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            session.fail_with_error(format!("VCP服务器错误: {} - {}", status, text))
        }
        Err(error) => {
            log::warn!(
                "[VCPClient] Connection failed, transitioning to Retrying: {:?}",
                error
            );
            StreamControl::Continue(State::Retrying)
        }
    }
}

pub(super) async fn resume<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    while !crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(&session.app) {
        log::info!(
            "[VCPClient] App is in background. Suspending reconnection for message: {}",
            session.message_id
        );
        tokio::select! {
            _ = &mut session.abort_rx => {
                #[cfg(target_os = "android")]
                session.stop_helper().await;
                return session.cancel_silently();
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }

    #[cfg(target_os = "android")]
    {
        resume_android(session).await
    }
    #[cfg(not(target_os = "android"))]
    {
        log::warn!(
            "[VCPClient] Reconnection is only supported on Android via SSE proxy. Transitioning to Aligning."
        );
        StreamControl::Continue(State::Aligning)
    }
}

#[cfg(target_os = "android")]
async fn resume_android<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let start_index = session
        .last_received_index
        .map(|index| index + 1)
        .unwrap_or(0);
    let params = json!({
        "startIndex": start_index,
        "ownerType": session.request_key.topic.owner_type,
        "ownerId": session.request_key.topic.owner_id,
        "topicId": session.request_key.topic.topic_id
    });
    match session.connect_helper("resume", Some(params)).await {
        Ok(stream) => {
            log::info!("[VCPClient] Successfully reconnected to sse helper socket");
            session.source = Some(StreamSource::Tcp(FramedRead::new(
                stream,
                LengthDelimitedCodec::new(),
            )));
            session.retry_count = 0;
            session.backoff = Duration::from_millis(500);
            StreamControl::Continue(State::Streaming)
        }
        Err(error) => {
            log::warn!("[VCPClient] Failed to reconnect to sse helper: {:?}", error);
            StreamControl::Continue(State::Aligning)
        }
    }
}

#[cfg(not(target_os = "android"))]
fn to_line_stream(response: reqwest::Response) -> super::LineStream {
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let framed = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));
    Box::new(framed.map_err(std::io::Error::other))
}
