use super::{State, StreamControl, StreamSession, StreamSource};
use serde_json::Value;
use tauri::Runtime;

enum Next<T> {
    Aborted,
    Missing,
    Item(T),
}

pub(super) async fn consume<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    #[cfg(target_os = "android")]
    {
        consume_android(session).await
    }
    #[cfg(not(target_os = "android"))]
    {
        consume_desktop(session).await
    }
}

#[cfg(target_os = "android")]
async fn consume_android<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    if !matches!(session.source, Some(StreamSource::Tcp(_))) {
        log::error!("[VCPClient] 流状态缺少 helper TCP 来源");
        session.stop_helper().await;
        return StreamControl::Continue(State::Retrying);
    }
    loop {
        match next_android(session).await {
            Next::Aborted => {
                session.stop_helper().await;
                return session.cancel_during_stream();
            }
            Next::Missing => {
                session.stop_helper().await;
                return StreamControl::Continue(State::Retrying);
            }
            Next::Item(None) => {
                log::warn!("[VCPClient] helper TCP socket 被服务端关闭，进入重试");
                if let Err(error) = session.prepare_helper_takeover().await {
                    log::warn!("[VCPClient] 准备 helper socket 接管失败：{error}");
                    session.stop_helper().await;
                }
                return StreamControl::Continue(State::Retrying);
            }
            Next::Item(Some(Err(error))) => {
                log::warn!("[VCPClient] helper TCP socket 读取失败：{:?}", error);
                if let Err(error) = session.prepare_helper_takeover().await {
                    log::warn!("[VCPClient] 准备 helper socket 接管失败：{error}");
                    session.stop_helper().await;
                }
                return StreamControl::Continue(State::Retrying);
            }
            Next::Item(Some(Ok(None))) => {}
            Next::Item(Some(Ok(Some(event)))) => match handle_android_event(session, event) {
                AndroidEvent::Continue => {}
                AndroidEvent::Complete => {
                    session.stop_helper().await;
                    return session.finish_success();
                }
                AndroidEvent::Error(error) => {
                    session.stop_helper().await;
                    return session.fail_with_error(error);
                }
            },
        }
    }
}

#[cfg(target_os = "android")]
enum AndroidEvent {
    Continue,
    Complete,
    Error(String),
}

#[cfg(target_os = "android")]
fn handle_android_event<R: Runtime>(session: &mut StreamSession<R>, event: Value) -> AndroidEvent {
    if !is_current_helper_event(session, &event) {
        return AndroidEvent::Continue;
    }
    if let Some(index) = event.get("index").and_then(Value::as_i64) {
        session.last_received_index = Some(index);
    }
    let event_type = event["eventType"].as_str().unwrap_or("");
    let event_data = event["eventData"].as_str().unwrap_or("");
    match event_type {
        "message" if event_data == "[DONE]" => AndroidEvent::Complete,
        "message" => handle_message_payload(session, event_data),
        "closed" => AndroidEvent::Complete,
        "error" => AndroidEvent::Error(proxy_error_message(event_data)),
        _ => AndroidEvent::Continue,
    }
}

#[cfg(target_os = "android")]
fn is_current_helper_event<R: Runtime>(session: &StreamSession<R>, event: &Value) -> bool {
    let Some(generation) = event.get("generation").and_then(Value::as_u64) else {
        log::error!("[VCPClient] helper 事件缺少 generation，已忽略");
        return false;
    };
    if session.helper_generation != Some(generation) {
        log::warn!(
            "[VCPClient] 忽略旧 helper generation 事件: expected={:?}, actual={generation}",
            session.helper_generation
        );
        return false;
    }
    for (field, expected) in [
        ("requestId", session.request_key.msg_id.as_str()),
        ("messageId", session.request_key.msg_id.as_str()),
        ("ownerType", session.request_key.topic.owner_type.as_str()),
        ("ownerId", session.request_key.topic.owner_id.as_str()),
        ("topicId", session.request_key.topic.topic_id.as_str()),
    ] {
        if event[field].as_str() != Some(expected) {
            log::error!("[VCPClient] helper 事件身份字段 {field} 不一致，已忽略");
            return false;
        }
    }
    true
}

#[cfg(target_os = "android")]
fn handle_message_payload<R: Runtime>(
    session: &mut StreamSession<R>,
    event_data: &str,
) -> AndroidEvent {
    if let Ok(payload) = serde_json::from_str::<Value>(event_data) {
        process_payload(session, &payload);
    }
    AndroidEvent::Continue
}

#[cfg(target_os = "android")]
fn proxy_error_message(event_data: &str) -> String {
    serde_json::from_str::<Value>(event_data)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| "本地代理发生未知错误".to_string())
}

#[cfg(target_os = "android")]
async fn next_android<R: Runtime>(
    session: &mut StreamSession<R>,
) -> Next<Option<Result<Option<Value>, std::io::Error>>> {
    let Some(StreamSource::Tcp(reader)) = session.source.as_mut() else {
        return Next::Missing;
    };
    tokio::select! {
        _ = &mut session.abort_rx => Next::Aborted,
        next = futures_util::StreamExt::next(reader) => Next::Item(next.map(|result| {
            result.map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        })),
    }
}

#[cfg(not(target_os = "android"))]
async fn consume_desktop<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    if !matches!(session.source, Some(StreamSource::Lines(_))) {
        log::warn!("[VCPClient] 流状态缺少文本来源");
        return StreamControl::Continue(State::Retrying);
    }
    loop {
        match next_desktop(session).await {
            Next::Aborted => return session.cancel_during_stream(),
            Next::Missing => return StreamControl::Continue(State::Retrying),
            Next::Item(Some(Err(error))) => {
                log::warn!("[VCPClient] 流读取失败：{:?}", error);
                return StreamControl::Continue(State::Retrying);
            }
            Next::Item(None) => {
                if session.aurora_buffer.full_text.is_empty()
                    && session.last_finish_reason.is_none()
                {
                    log::warn!("[VCPClient] 流意外结束，进入重试");
                    return StreamControl::Continue(State::Retrying);
                }
                return session.finish_success();
            }
            Next::Item(Some(Ok(line))) => {
                if handle_sse_line(session, &line) {
                    return session.finish_success();
                }
            }
        }
    }
}

#[cfg(not(target_os = "android"))]
async fn next_desktop<R: Runtime>(
    session: &mut StreamSession<R>,
) -> Next<Option<Result<String, std::io::Error>>> {
    let Some(StreamSource::Lines(lines)) = session.source.as_mut() else {
        return Next::Missing;
    };
    tokio::select! {
        _ = &mut session.abort_rx => Next::Aborted,
        next = futures_util::StreamExt::next(lines) => Next::Item(next),
    }
}

#[cfg(not(target_os = "android"))]
fn handle_sse_line<R: Runtime>(session: &mut StreamSession<R>, line: &str) -> bool {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return false;
    };
    if data == "[DONE]" {
        return true;
    }
    if let Ok(payload) = serde_json::from_str::<Value>(data) {
        process_payload(session, &payload);
    }
    false
}

fn process_payload<R: Runtime>(session: &mut StreamSession<R>, payload: &Value) {
    if let Some(reason) = payload.get("finish_reason").and_then(Value::as_str) {
        session.last_finish_reason = Some(reason.to_string());
    }
    let Some(delta) = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(Value::as_str)
    else {
        return;
    };
    session.pending_aurora_chunk.push_str(delta);
    let (stable_changed, tail_changed) = session.flush_aurora_parse(false);
    let has_mutations = !session.aurora_buffer.pending_mutations.is_empty();
    if stable_changed || tail_changed || has_mutations {
        session.send_aurora_update(stable_changed, tail_changed, None, None);
    }
}
