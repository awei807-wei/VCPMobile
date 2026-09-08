use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat::message_service;
use crate::vcp_modules::chat::topic_service::TempMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::vcp_client::{
    acquire_stream_service, mark_message_as_error_guarded_with_channel, perform_vcp_request,
    ActiveRequests, GuardedTransition, StreamEvent, VcpRequestError, VcpRequestOutcome,
    VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, Manager, State};
use tauri_plugin_vcp_mobile::stream::StreamIdentity;

use super::AssistantChatActivityState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantChatPayload {
    pub agent_id: String,
    pub temp_messages: Vec<TempMessage>,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[tauri::command]
pub async fn handle_assistant_chat_stream(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AssistantChatPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    let activity_guard = app_handle
        .try_state::<AssistantChatActivityState>()
        .map(|state| state.begin(&app_handle));
    let AssistantChatPayload {
        agent_id,
        temp_messages,
        vcp_url,
        vcp_api_key,
    } = payload;
    let (thinking_id, agent_config, request_payload) = prepare_assistant_request(
        &app_handle,
        &agent_state,
        &agent_id,
        temp_messages,
        vcp_url,
        vcp_api_key,
    )
    .await?;
    let stream_identity = StreamIdentity::new("agent", &agent_id, "assistant_chat", &thinking_id);
    let stream_lease = acquire_stream_service(
        &app_handle,
        &agent_config.name,
        &stream_identity,
        "AssistantChatAppService",
    );
    let result = perform_vcp_request(
        &app_handle,
        active_requests.0.clone(),
        request_payload,
        Some(stream_channel.clone()),
    )
    .await;
    let db_state = app_handle
        .try_state::<DbState>()
        .ok_or_else(|| "核心数据库尚未就绪".to_string())?;
    let emitted = emit_assistant_result(result, &app_handle, &db_state.pool, &stream_channel).await;
    drop(stream_lease);
    emitted?;
    drop(activity_guard);
    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}

async fn prepare_assistant_request(
    app_handle: &AppHandle,
    agent_state: &State<'_, AgentConfigState>,
    agent_id: &str,
    temp_messages: Vec<TempMessage>,
    vcp_url: String,
    vcp_api_key: String,
) -> Result<(String, AgentConfig, VcpRequestPayload), String> {
    let thinking_id = new_thinking_id(agent_id);
    let agent_config =
        read_agent_config_internal(app_handle, agent_state, agent_id, Some(true)).await?;
    let context = Some(json!({
        "agentId": agent_id,
        "ownerType": "agent",
        "topicId": "assistant_chat",
        "agentName": agent_config.name
    }));
    Ok((
        thinking_id.clone(),
        agent_config.clone(),
        VcpRequestPayload {
            vcp_url,
            vcp_api_key,
            messages: build_messages(&agent_config, temp_messages),
            model_config: build_model_config(&agent_config),
            message_id: thinking_id,
            context,
        },
    ))
}

fn build_messages(agent_config: &AgentConfig, temp_messages: Vec<TempMessage>) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": effective_system_prompt(agent_config)
    })];
    messages.extend(
        temp_messages
            .into_iter()
            .map(|message| json!({ "role": message.role, "content": message.content })),
    );
    messages
}

async fn emit_assistant_result(
    result: Result<VcpRequestOutcome, VcpRequestError>,
    app_handle: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    stream_channel: &Channel<StreamEvent>,
) -> Result<(), String> {
    match result {
        Ok(outcome) => emit_assistant_success(outcome, app_handle, pool, stream_channel).await,
        Err(error) => emit_assistant_error(error, app_handle, pool, stream_channel).await,
    }
}

async fn emit_assistant_success(
    outcome: VcpRequestOutcome,
    app_handle: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    stream_channel: &Channel<StreamEvent>,
) -> Result<(), String> {
    let VcpRequestOutcome {
        response,
        is_aborted,
        completion_lease,
        request_guard,
    } = outcome;
    let _request_guard = request_guard;
    let Some(full_content) = response["fullContent"].as_str() else {
        let mark_result = mark_message_as_error_guarded_with_channel(
            app_handle,
            pool,
            &completion_lease,
            Some("响应缺少 fullContent".to_string()),
            Some(stream_channel.clone()),
        )
        .await?;
        return match mark_result {
            GuardedTransition::Applied(()) => Err("响应缺少 fullContent".to_string()),
            GuardedTransition::Skipped => {
                log::warn!("[AssistantChatAppService] 缺少正文的旧请求已跳过");
                Ok(())
            }
        };
    };
    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        response["finishReason"].as_str().map(str::to_string)
    };
    let finalization = message_service::finalize_stream_message_guarded(
        app_handle.clone(),
        pool,
        &completion_lease,
        full_content.to_string(),
        is_aborted,
        finish_reason,
        Some(stream_channel.clone()),
        Some(completion_lease.key().topic.owner_id.clone()),
    )
    .await?;
    if matches!(
        finalization,
        message_service::StreamFinalizationStatus::Skipped
    ) {
        log::warn!("[AssistantChatAppService] 旧请求终结已跳过");
    }
    Ok(())
}

async fn emit_assistant_error(
    error: VcpRequestError,
    app_handle: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    stream_channel: &Channel<StreamEvent>,
) -> Result<(), String> {
    let VcpRequestError {
        message: error_message,
        completion_lease,
        stale,
        request_guard,
    } = error;
    let _request_guard = request_guard;
    if stale {
        log::warn!("[AssistantChatAppService] 旧请求已被新请求替换");
        return Ok(());
    }
    let Some(lease) = completion_lease else {
        log::error!(
            "[AssistantChatAppService] VCP 请求执行失败: {}",
            error_message
        );
        return Err(error_message);
    };
    let transition = {
        let mark_transition = mark_message_as_error_guarded_with_channel(
            app_handle,
            pool,
            &lease,
            Some(error_message.clone()),
            Some(stream_channel.clone()),
        )
        .await?;
        if matches!(mark_transition, GuardedTransition::Skipped) {
            log::warn!("[AssistantChatAppService] 请求错误终结已跳过旧请求");
        }
        mark_transition
    };
    if matches!(transition, GuardedTransition::Skipped) {
        log::warn!("[AssistantChatAppService] 旧请求 error 事件已跳过");
    }
    log::error!(
        "[AssistantChatAppService] VCP 请求执行失败: {}",
        error_message
    );
    Err(error_message)
}

fn build_model_config(agent_config: &AgentConfig) -> Value {
    let mut config = json!({
        "model": agent_config.model,
        "max_tokens": agent_config.max_output_tokens,
        "contextTokenLimit": agent_config.context_token_limit,
        "stream": true
    });
    if agent_config.use_temperature {
        config["temperature"] = json!(agent_config.temperature);
    }
    config
}

fn effective_system_prompt(agent_config: &AgentConfig) -> String {
    if agent_config.mobile_system_prompt.is_empty() {
        agent_config.system_prompt.clone()
    } else {
        agent_config.mobile_system_prompt.clone()
    }
}

fn new_thinking_id(agent_id: &str) -> String {
    let timestamp = crate::vcp_modules::infra::utils::now_millis();
    format!("msg_{}_{}", agent_id, timestamp)
}
