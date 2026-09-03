use super::active::mark_message_as_error;
use super::{
    db_pool_if_ready, handle_streaming_request, ActiveRequestGuard, ActiveRequests, StreamEvent,
};
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, Runtime};
use tokio::sync::oneshot;

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
) -> Result<Value, String> {
    let request_key =
        super::registry::message_key_from_parts(&owner_id, &owner_type, &topic_id, &msg_id)?;
    log::info!(
        "[VCPClient] resume_stream called for messageId: {}, topicId: {}, lastEventIndex: {:?}",
        msg_id,
        topic_id,
        last_event_index
    );

    // 与 get/recover 保持相同的冷启动边界：核心未就绪时只返回可重试错误。
    let pool = db_pool_if_ready(&app)?;
    persist_initial_content(&pool, &request_key, &msg_id, initial_content.as_deref()).await?;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| e.to_string())?;
    let (abort_rx, request_epoch, _guard) = register_resume_request(&state, &request_key);
    let context = build_resume_context(&owner_id, &owner_type, &topic_id);
    let (res, is_aborted) = execute_resume_request(
        &app,
        &pool,
        client,
        msg_id.clone(),
        request_key.clone(),
        request_epoch,
        context.clone(),
        abort_rx,
        state.0.clone(),
        stream_channel.clone(),
        last_event_index,
        initial_content.clone(),
    )
    .await?;
    finalize_resumed_message(
        &app,
        &pool,
        &owner_id,
        &owner_type,
        &topic_id,
        &msg_id,
        &res,
        is_aborted,
        stream_channel,
    )
    .await?;
    Ok(res)
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
) -> Result<(Value, bool), String> {
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
    )
    .await
    {
        Ok(value) => Ok(value),
        Err(error) => {
            log::error!(
                "[VCPClient] resume_stream failed during handle_streaming_request: {}",
                error
            );
            let _ = mark_message_as_error(
                app,
                pool,
                &request_key,
                Some(format!("接续失败: {}", error)),
            )
            .await;
            Err(error)
        }
    }
}

async fn persist_initial_content(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &crate::vcp_modules::chat::topic_types::MessageKey,
    msg_id: &str,
    content: Option<&str>,
) -> Result<(), String> {
    let Some(content) = content else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE messages SET content = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(ContentCompressor::compress(content)?)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(msg_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|error| format!("恢复流式消息正文失败: {error}"))
}

fn register_resume_request(
    state: &tauri::State<'_, ActiveRequests>,
    key: &crate::vcp_modules::chat::topic_types::MessageKey,
) -> (oneshot::Receiver<()>, u64, ActiveRequestGuard) {
    let (abort_tx, abort_rx) = oneshot::channel();
    let (request_epoch, previous_sender) = state.0.register(key.clone(), abort_tx);
    if let Some(previous_sender) = previous_sender {
        let _ = previous_sender.send(());
    }
    let guard = ActiveRequestGuard::new(state.0.clone(), key.clone(), request_epoch);
    (abort_rx, request_epoch, guard)
}

fn build_resume_context(owner_id: &str, owner_type: &str, topic_id: &str) -> Value {
    json!({
        "topicId": topic_id,
        "ownerType": owner_type,
        "groupId": if owner_type == "group" { Some(owner_id) } else { None::<&str> },
        "agentId": if owner_type == "agent" { Some(owner_id) } else { None::<&str> },
    })
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
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finalize_resumed_message<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    msg_id: &str,
    response: &Value,
    is_aborted: bool,
    stream_channel: Channel<StreamEvent>,
) -> Result<(), String> {
    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        response["finishReason"].as_str().map(str::to_string)
    };
    log::info!("[VCPClient] resume_stream completed. Finalizing message.");
    crate::vcp_modules::chat::message_service::finalize_stream_message(
        app.clone(),
        pool,
        owner_id,
        owner_type,
        topic_id.to_string(),
        msg_id.to_string(),
        response["fullContent"].as_str().unwrap_or("").to_string(),
        is_aborted,
        finish_reason,
        Some(stream_channel),
        (owner_type == "agent").then(|| owner_id.to_string()),
    )
    .await
}
