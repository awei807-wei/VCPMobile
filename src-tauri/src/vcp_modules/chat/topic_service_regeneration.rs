use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::Value;
use sqlx::Row;
use tauri::{ipc::Channel, AppHandle, State};

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn regenerate_topic_response(
    app_handle: AppHandle,
    agent_state: State<'_, crate::vcp_modules::agent_service::AgentConfigState>,
    group_state: State<'_, crate::vcp_modules::group_service::GroupManagerState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    cancelled_turns: State<'_, crate::vcp_modules::vcp_client::CancelledGroupTurns>,
    settings_state: State<'_, SettingsState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    target_user_msg_id: String,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    log::info!(
        "[TopicService] Regenerating response for topic: {}, target msg: {}",
        topic_id,
        target_user_msg_id
    );
    let topic_key = TopicKey::new(owner_type.clone(), owner_id.clone(), topic_id.clone());
    let (chat_msg, timestamp) =
        load_regeneration_message(&app_handle, &db_state.pool, &topic_key, &target_user_msg_id)
            .await?;
    crate::vcp_modules::message_service::truncate_history_after_timestamp_for_topic(
        &db_state.pool,
        &topic_key,
        timestamp,
    )
    .await?;
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await?;
    dispatch_regeneration(
        app_handle,
        agent_state,
        group_state,
        db_state,
        active_requests,
        cancelled_turns,
        owner_id,
        owner_type,
        topic_id,
        chat_msg,
        settings,
        stream_channel,
    )
    .await
}

async fn load_regeneration_message(
    app_handle: &AppHandle,
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    message_id: &str,
) -> Result<(ChatMessage, i64), String> {
    let content = crate::vcp_modules::message_service::fetch_raw_message_content_for_key(
        app_handle,
        &topic_key.owner_type,
        &topic_key.owner_id,
        &topic_key.topic_id,
        message_id,
    )
    .await?;
    let row = sqlx::query(
        "SELECT timestamp, role, name, agent_id, group_id, is_group_message
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .bind(message_id)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    let timestamp: i64 = row.get("timestamp");
    let message = ChatMessage {
        id: message_id.to_string(),
        role: row.get("role"),
        name: row.get("name"),
        content,
        timestamp: timestamp as u64,
        updated_at: Some(timestamp as u64),
        is_thinking: Some(false),
        agent_id: row.get("agent_id"),
        group_id: row.get("group_id"),
        topic_id: Some(topic_key.topic_id.clone()),
        is_group_message: Some(row.get::<i64, _>("is_group_message") != 0),
        finish_reason: None,
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    };
    Ok((message, timestamp))
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_regeneration(
    app_handle: AppHandle,
    agent_state: State<'_, crate::vcp_modules::agent_service::AgentConfigState>,
    group_state: State<'_, crate::vcp_modules::group_service::GroupManagerState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    cancelled_turns: State<'_, crate::vcp_modules::vcp_client::CancelledGroupTurns>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    chat_msg: ChatMessage,
    settings: crate::vcp_modules::settings_manager::Settings,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    if owner_type == "agent" {
        crate::vcp_modules::agent_chat_application_service::internal_process_agent_chat_message(
            app_handle,
            agent_state,
            db_state,
            active_requests,
            crate::vcp_modules::agent_chat_application_service::AgentChatPayload {
                agent_id: owner_id,
                topic_id: topic_id.clone(),
                user_message: chat_msg,
                vcp_url: settings.vcp_server_url,
                vcp_api_key: settings.vcp_api_key,
            },
            stream_channel,
            false,
        )
        .await
    } else {
        crate::vcp_modules::group_chat_application_service::internal_process_group_chat_message(
            app_handle,
            group_state,
            agent_state,
            db_state,
            active_requests,
            cancelled_turns,
            crate::vcp_modules::group_chat_application_service::GroupChatParams {
                group_id: owner_id,
                topic_id: topic_id.clone(),
                user_message: chat_msg,
                vcp_url: settings.vcp_server_url,
                vcp_api_key: settings.vcp_api_key,
                stream_channel: Some(stream_channel),
            },
            false,
        )
        .await
    }
}
