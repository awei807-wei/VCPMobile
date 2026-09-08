// 群聊应用服务入口，具体回合编排位于 flow 模块。

use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::vcp_client::{ActiveRequests, CancelledGroupTurns, StreamEvent};
use serde::Deserialize;
use serde_json::Value;
use tauri::{ipc::Channel, AppHandle, State};

#[path = "group_chat_application_service_flow.rs"]
mod flow;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupChatPayload {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
}

pub struct GroupChatParams {
    pub group_id: String,
    pub topic_id: String,
    pub user_message: ChatMessage,
    pub vcp_url: String,
    pub vcp_api_key: String,
    pub stream_channel: Option<Channel<StreamEvent>>,
}

#[allow(clippy::too_many_arguments)]
pub async fn internal_process_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    params: GroupChatParams,
    append_user_msg: bool,
) -> Result<Value, String> {
    flow::process_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        active_requests,
        cancelled_turns,
        params,
        append_user_msg,
    )
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn handle_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    payload: GroupChatPayload,
    stream_channel: Channel<StreamEvent>,
) -> Result<Value, String> {
    internal_process_group_chat_message(
        app_handle,
        group_state,
        agent_state,
        db_state,
        active_requests,
        cancelled_turns,
        GroupChatParams {
            group_id: payload.group_id,
            topic_id: payload.topic_id,
            user_message: payload.user_message,
            vcp_url: payload.vcp_url,
            vcp_api_key: payload.vcp_api_key,
            stream_channel: Some(stream_channel),
        },
        false,
    )
    .await
}
