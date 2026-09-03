use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat::topic_service::TempMessage;
use crate::vcp_modules::vcp_client::{
    perform_vcp_request, ActiveRequests, StreamEvent, VcpRequestPayload,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{ipc::Channel, AppHandle, Manager, State};

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
    let thinking_id = new_thinking_id(&agent_id);
    let agent_config =
        read_agent_config_internal(&app_handle, &agent_state, &agent_id, Some(true)).await?;
    start_stream_service(&app_handle, &agent_config.name);
    let messages = build_messages(&agent_config, temp_messages);
    let context = Some(json!({
        "agentId": agent_id,
        "topicId": "assistant_chat",
        "agentName": agent_config.name
    }));
    let request_payload = VcpRequestPayload {
        vcp_url,
        vcp_api_key,
        messages,
        model_config: build_model_config(&agent_config),
        message_id: thinking_id.clone(),
        context: context.clone(),
    };
    let _ = stream_channel.send(StreamEvent::thinking(thinking_id.clone(), context.clone()));
    let result = perform_vcp_request(
        &app_handle,
        active_requests.0.clone(),
        request_payload,
        Some(stream_channel.clone()),
    )
    .await;
    stop_stream_service(&app_handle, &agent_config.name);
    emit_assistant_result(result, &stream_channel, &thinking_id, context).await;
    drop(activity_guard);
    Ok(json!({ "status": "sent", "messageId": thinking_id }))
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
    result: Result<(Value, bool), String>,
    stream_channel: &Channel<StreamEvent>,
    thinking_id: &str,
    context: Option<Value>,
) {
    match result {
        Ok((response, is_aborted)) if response["fullContent"].is_string() => {
            let finish_reason = if is_aborted {
                Some("cancelled_by_user".to_string())
            } else {
                response["finishReason"].as_str().map(str::to_string)
            };
            let timestamp = crate::vcp_modules::infra::utils::now_millis() as u64;
            let _ = stream_channel.send(StreamEvent::end(
                thinking_id.to_string(),
                context,
                finish_reason,
                None,
                Some(timestamp),
            ));
        }
        Ok(_) => {}
        Err(error) => {
            log::error!(
                "[AssistantChatAppService] perform_vcp_request failed: {}",
                error
            );
            let _ =
                stream_channel.send(StreamEvent::error(thinking_id.to_string(), context, error));
        }
    }
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

fn start_stream_service(app_handle: &AppHandle, agent_name: &str) {
    if let Err(error) =
        tauri_plugin_vcp_mobile::stream::start_stream_service_inner(app_handle, agent_name)
    {
        log::warn!(
            "[AssistantChatAppService] Failed to start streaming service early: {}",
            error
        );
    }
}

fn stop_stream_service(app_handle: &AppHandle, agent_name: &str) {
    if let Err(error) =
        tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(app_handle, agent_name)
    {
        log::warn!(
            "[AssistantChatAppService] Failed to stop streaming service: {}",
            error
        );
    }
}
