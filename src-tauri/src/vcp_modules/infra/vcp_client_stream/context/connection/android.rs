use super::super::super::super::{
    db_pool_if_ready, transport, ActiveRequestRegistry, GuardedTransition,
};
use super::super::{State, StreamControl, StreamSession, StreamSource};
use super::bind::{bind_generation_with_timeout_or_cancel, BindGenerationAttempt};
use super::stop::{
    helper_stop_context, stop_helper_generation_with_context, verify_generation_ack,
};
use crate::vcp_modules::chat::topic_types::MessageKey;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tauri::Runtime;
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

const HELPER_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

enum HelperConnectError {
    Cancelled,
    Failed(String),
}

pub(super) async fn connect_android<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let headers = json!({
        "Authorization": format!("Bearer {}", session.api_key),
        "Content-Type": "application/json"
    });
    let params = json!({
        "url": session.final_url,
        "headers": headers.to_string(),
        "body": session.request_body.to_string(),
        "context": build_sse_context(session),
        "requestEpoch": session.request_epoch
    });
    match connect_helper_with_generation(session, "start", Some(params), None).await {
        Ok((reader, generation)) => {
            session.helper_generation = Some(generation);
            session.source = Some(StreamSource::Tcp(reader));
            StreamControl::Continue(State::Streaming)
        }
        Err(HelperConnectError::Cancelled) => session.cancel_before_streaming(),
        Err(HelperConnectError::Failed(error)) => {
            log::error!("[VCPClient] 连接 helper 失败：{:?}", error);
            session.fail_with_error(format!("启动本地代理失败: {}", error))
        }
    }
}

fn build_sse_context<R: Runtime>(session: &StreamSession<R>) -> Value {
    let mut context = json!({
        "ownerType": session.request_key.topic.owner_type,
        "ownerId": session.request_key.topic.owner_id,
        "topicId": session.request_key.topic.topic_id,
        "messageId": session.request_key.msg_id
    });
    if let Some(agent_name) = session
        .context
        .as_ref()
        .and_then(|value| value.get("agentName"))
        .and_then(Value::as_str)
    {
        context["agentName"] = json!(agent_name);
    }
    context
}

async fn connect_helper_with_generation<R: Runtime>(
    session: &mut StreamSession<R>,
    command: &str,
    params: Option<Value>,
    expected_generation: Option<u64>,
) -> Result<(super::super::TcpReader, u64), HelperConnectError> {
    let (reader, generation) =
        read_helper_handshake(session, command, params, expected_generation).await?;
    bind_helper_generation_with_timeout(session, generation).await?;
    session
        .helper_stop_generation
        .store(0, std::sync::atomic::Ordering::Release);
    let parts = reader.into_parts();
    let mut restored = FramedRead::with_capacity(parts.io, parts.codec, parts.read_buf.len());
    restored
        .read_buffer_mut()
        .extend_from_slice(&parts.read_buf);
    Ok((restored, generation))
}

async fn read_helper_handshake<R: Runtime>(
    session: &mut StreamSession<R>,
    command: &str,
    params: Option<Value>,
    expected_generation: Option<u64>,
) -> Result<(super::super::TcpReader, u64), HelperConnectError> {
    let app = session.app.clone();
    let request_key = session.request_key.clone();
    let connect = transport::connect_to_helper(&app, command, &request_key, params);
    tokio::pin!(connect);
    let stream = tokio::select! {
        biased;
        _ = &mut session.abort_rx => return Err(HelperConnectError::Cancelled),
        result = &mut connect => result.map_err(HelperConnectError::Failed)?,
    };
    let mut reader = FramedRead::new(stream, LengthDelimitedCodec::new());
    let frame = tokio::select! {
        _ = &mut session.abort_rx => return Err(HelperConnectError::Cancelled),
        result = tokio::time::timeout(HELPER_HANDSHAKE_TIMEOUT, reader.next()) => {
            let frame = result
                .map_err(|_| HelperConnectError::Failed("helper generation 握手超时".to_string()))?
                .ok_or_else(|| HelperConnectError::Failed("helper 未返回 generation 握手".to_string()))?;
            frame.map_err(|error| HelperConnectError::Failed(format!("读取 helper generation 握手失败: {error}")))?
        }
    };
    let response = serde_json::from_slice::<Value>(&frame).map_err(|error| {
        HelperConnectError::Failed(format!("解析 helper generation 握手失败: {error}"))
    })?;
    let generation = response["generation"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            HelperConnectError::Failed("helper generation 握手缺少有效 generation".to_string())
        })?;
    if let Err(error) = verify_generation_ack(session, &response) {
        super::stop::stop_helper_generation(session, generation).await;
        return Err(HelperConnectError::Failed(error));
    }
    if let Some(expected_generation) = expected_generation {
        if generation != expected_generation {
            super::stop::stop_helper_generation(session, generation).await;
            return Err(HelperConnectError::Failed(format!(
                "helper resume ack generation 不匹配: expected={expected_generation}, actual={generation}"
            )));
        }
    }
    Ok((reader, generation))
}

async fn bind_helper_generation_with_timeout<R: Runtime>(
    session: &mut StreamSession<R>,
    generation: u64,
) -> Result<(), HelperConnectError> {
    let bind_app = session.app.clone();
    let bind_requests = session.active_requests.clone();
    let bind_key = session.request_key.clone();
    let bind_epoch = session.request_epoch;
    let stop_context = helper_stop_context(session);
    let stop_context_for_bind = stop_context;
    let bind_result = bind_generation_with_timeout_or_cancel(
        HELPER_HANDSHAKE_TIMEOUT,
        &mut session.abort_rx,
        bind_helper_generation(&bind_app, &bind_requests, &bind_key, bind_epoch, generation),
        move || {
            let stop_context = stop_context_for_bind.clone();
            async move {
                stop_helper_generation_with_context(stop_context, generation).await;
            }
        },
    )
    .await;
    match bind_result {
        BindGenerationAttempt::Bound => {}
        BindGenerationAttempt::Cancelled => return Err(HelperConnectError::Cancelled),
        BindGenerationAttempt::Failed(error) => {
            return Err(HelperConnectError::Failed(error));
        }
    }
    Ok(())
}

async fn bind_helper_generation<R: Runtime>(
    app: &tauri::AppHandle<R>,
    active_requests: &Arc<ActiveRequestRegistry>,
    request_key: &MessageKey,
    request_epoch: u64,
    generation: u64,
) -> Result<(), String> {
    let pool = db_pool_if_ready(app)?;
    let key = request_key.clone();
    let registry_key = key.clone();
    let generation_i64 = i64::try_from(generation)
        .map_err(|_| "helper generation 超出 SQLite 整数范围".to_string())?;
    let transition = active_requests
        .bind_session_generation_with(&registry_key, request_epoch, generation, || async move {
            let result = sqlx::query(
                "UPDATE active_generations
                     SET helper_generation = ?
                     WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
                       AND (helper_generation IS NULL OR helper_generation = ?)",
            )
            .bind(generation_i64)
            .bind(&key.topic.owner_type)
            .bind(&key.topic.owner_id)
            .bind(&key.topic.topic_id)
            .bind(&key.msg_id)
            .bind(generation_i64)
            .execute(&pool)
            .await
            .map_err(|error| format!("持久化 helper generation 失败: {error}"))?;
            if result.rows_affected() != 1 {
                return Err("active_generations 不存在或 generation 已不一致".to_string());
            }
            Ok(())
        })
        .await?;
    match transition {
        GuardedTransition::Applied(()) => Ok(()),
        GuardedTransition::Skipped => Err("请求已被替换，拒绝绑定 helper generation".to_string()),
    }
}

pub(super) async fn resume_android<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let start_index = session
        .last_received_index
        .map(|index| index + 1)
        .unwrap_or(0);
    let params = json!({
        "startIndex": start_index,
        "expectedGeneration": session.helper_generation,
        "ownerType": session.request_key.topic.owner_type,
        "ownerId": session.request_key.topic.owner_id,
        "topicId": session.request_key.topic.topic_id
    });
    let Some(expected_generation) = session.helper_generation else {
        return session.fail_with_error("接续缺少当前 helper generation".to_string());
    };
    match connect_helper_with_generation(session, "resume", Some(params), Some(expected_generation))
        .await
    {
        Ok((reader, generation)) => {
            log::info!("[VCPClient] 已重新连接到 SSE helper socket");
            if generation != expected_generation {
                return session.fail_with_error("helper resume generation 被意外覆盖".to_string());
            }
            session.source = Some(StreamSource::Tcp(reader));
            session.retry_count = 0;
            session.backoff = Duration::from_millis(500);
            StreamControl::Continue(State::Streaming)
        }
        Err(HelperConnectError::Cancelled) => {
            super::stop::stop_helper_generation(session, expected_generation).await;
            session.cancel_silently()
        }
        Err(HelperConnectError::Failed(error)) => {
            log::warn!("[VCPClient] 重新连接 SSE helper 失败：{:?}", error);
            StreamControl::Continue(State::Aligning)
        }
    }
}
