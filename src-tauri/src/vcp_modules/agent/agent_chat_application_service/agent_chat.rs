use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::message_service;
use crate::vcp_modules::vcp_client::{
    acquire_stream_service, mark_message_as_error_guarded_with_channel, perform_vcp_request,
    ActiveRequests, GuardedTransition, StreamEvent, VcpRequestError, VcpRequestOutcome,
    VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, State};
use tauri_plugin_vcp_mobile::stream::StreamIdentity;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatPayload {
    pub agent_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

#[tauri::command]
pub async fn handle_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    internal_process_agent_chat_message(
        app_handle,
        agent_state,
        db_state,
        active_requests,
        payload,
        stream_channel,
        false,
    )
    .await
}

pub async fn internal_process_agent_chat_message(
    app_handle: AppHandle,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    payload: AgentChatPayload,
    stream_channel: Channel<StreamEvent>,
    append_user_msg: bool,
) -> Result<Value, String> {
    let AgentChatPayload {
        agent_id,
        topic_id,
        user_message,
        vcp_url,
        vcp_api_key,
    } = payload;
    let thinking_id = new_thinking_id(&agent_id);
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;
    let stream_identity = StreamIdentity::new("agent", &agent_id, &topic_id, &thinking_id);
    let stream_lease = acquire_stream_service(
        &app_handle,
        &agent_config.name,
        &stream_identity,
        "AgentChatAppService",
    );
    let request_payload = prepare_agent_request(AgentRequestInput {
        app_handle: &app_handle,
        db_state: &db_state,
        agent_id: &agent_id,
        topic_id: &topic_id,
        user_message,
        append_user_msg,
        agent_config: &agent_config,
        thinking_id: thinking_id.clone(),
        vcp_url,
        vcp_api_key,
    })
    .await?;
    let result = perform_vcp_request(
        &app_handle,
        active_requests.0.clone(),
        request_payload,
        Some(stream_channel.clone()),
    )
    .await;
    let finalized = finalize_agent_result(result, &app_handle, &db_state, stream_channel).await;
    drop(stream_lease);
    finalized?;
    Ok(json!({ "status": "sent", "messageId": thinking_id }))
}

struct AgentRequestInput<'a> {
    app_handle: &'a AppHandle,
    db_state: &'a DbState,
    agent_id: &'a str,
    topic_id: &'a str,
    user_message: ChatMessage,
    append_user_msg: bool,
    agent_config: &'a AgentConfig,
    thinking_id: String,
    vcp_url: String,
    vcp_api_key: String,
}

async fn prepare_agent_request(input: AgentRequestInput<'_>) -> Result<VcpRequestPayload, String> {
    let AgentRequestInput {
        app_handle,
        db_state,
        agent_id,
        topic_id,
        user_message,
        append_user_msg,
        agent_config,
        thinking_id,
        vcp_url,
        vcp_api_key,
    } = input;
    append_user_message(
        app_handle,
        db_state,
        agent_id,
        topic_id,
        user_message,
        append_user_msg,
    )
    .await?;
    let history = message_service::load_chat_history_internal(
        app_handle, agent_id, "agent", topic_id, None, None, true, true,
    )
    .await?;
    let messages =
        build_agent_context(db_state, &history, agent_id, topic_id, agent_config).await?;
    let context = Some(json!({
        "agentId": agent_id,
        "ownerType": "agent",
        "topicId": topic_id,
        "agentName": agent_config.name
    }));
    Ok(VcpRequestPayload {
        vcp_url,
        vcp_api_key,
        messages,
        model_config: build_model_config(agent_config),
        message_id: thinking_id,
        context,
    })
}

async fn append_user_message(
    app_handle: &AppHandle,
    db_state: &DbState,
    agent_id: &str,
    topic_id: &str,
    user_message: ChatMessage,
    append_user_msg: bool,
) -> Result<(), String> {
    if !append_user_msg {
        return Ok(());
    }
    message_service::append_single_message(
        app_handle.clone(),
        &db_state.pool,
        agent_id,
        "agent",
        topic_id.to_string(),
        user_message,
    )
    .await
    .map(|_| ())
}

async fn build_agent_context(
    db_state: &DbState,
    history: &[ChatMessage],
    agent_id: &str,
    topic_id: &str,
    agent_config: &AgentConfig,
) -> Result<Vec<Value>, String> {
    let effective_prompt = effective_system_prompt(agent_config);
    crate::vcp_modules::context_assembler::orchestrate_chat_context(
        crate::vcp_modules::context_assembler::ChatContextRequest {
            pool: &db_state.pool,
            history,
            owner_id: agent_id,
            topic_id,
            agent_name: &agent_config.name,
            scope: "agent",
            base_system_prompt: effective_prompt,
            invite_prompt: None,
        },
    )
    .await
}

async fn finalize_agent_result(
    result: Result<VcpRequestOutcome, VcpRequestError>,
    app_handle: &AppHandle,
    db_state: &DbState,
    stream_channel: Channel<StreamEvent>,
) -> Result<(), String> {
    match result {
        Ok(outcome) => finalize_agent_success(outcome, app_handle, db_state, stream_channel).await,
        Err(error) => finalize_agent_error(error, app_handle, db_state, stream_channel).await,
    }
}

async fn finalize_agent_success(
    outcome: VcpRequestOutcome,
    app_handle: &AppHandle,
    db_state: &DbState,
    stream_channel: Channel<StreamEvent>,
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
            &db_state.pool,
            &completion_lease,
            Some("响应缺少 fullContent".to_string()),
            Some(stream_channel),
        )
        .await?;
        return match mark_result {
            GuardedTransition::Applied(()) => Err("响应缺少 fullContent".to_string()),
            GuardedTransition::Skipped => {
                log::warn!("[AgentChatAppService] 缺少正文的旧请求已跳过");
                Ok(())
            }
        };
    };
    let finalization = message_service::finalize_stream_message_guarded(
        app_handle.clone(),
        &db_state.pool,
        &completion_lease,
        full_content.to_string(),
        is_aborted,
        finish_reason(&response, is_aborted),
        Some(stream_channel),
        Some(completion_lease.key().topic.owner_id.clone()),
    )
    .await?;
    if matches!(
        finalization,
        message_service::StreamFinalizationStatus::Skipped
    ) {
        log::warn!("[AgentChatAppService] 旧请求终结已跳过");
    }
    Ok(())
}

async fn finalize_agent_error(
    error: VcpRequestError,
    app_handle: &AppHandle,
    db_state: &DbState,
    stream_channel: Channel<StreamEvent>,
) -> Result<(), String> {
    let VcpRequestError {
        message,
        completion_lease,
        stale,
        request_guard,
    } = error;
    let _request_guard = request_guard;
    if stale {
        log::warn!("[AgentChatAppService] 旧请求已被新请求替换");
        return Ok(());
    }
    if let Some(lease) = completion_lease.as_ref() {
        let mark_result = mark_message_as_error_guarded_with_channel(
            app_handle,
            &db_state.pool,
            lease,
            Some(message.clone()),
            Some(stream_channel),
        )
        .await?;
        if matches!(mark_result, GuardedTransition::Skipped) {
            log::warn!("[AgentChatAppService] 请求错误终结已跳过旧请求");
            return Ok(());
        }
    }
    log::error!("[AgentChatAppService] 请求执行失败: {}", message);
    Err(message)
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

fn finish_reason(result: &Value, is_aborted: bool) -> Option<String> {
    if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        result["finishReason"].as_str().map(str::to_string)
    }
}
