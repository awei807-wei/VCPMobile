use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::settings_manager::SettingsState;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::{json, Value};
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
    target_response_msg_id: String,
    stream_channel: Channel<crate::vcp_modules::vcp_client::StreamEvent>,
) -> Result<Value, String> {
    let topic_key = TopicKey::new(owner_type.clone(), owner_id.clone(), topic_id.clone());
    let (chat_msg, anchor_message_id) =
        load_regeneration_message(&db_state.pool, &topic_key, &target_response_msg_id).await?;
    let captured = crate::vcp_modules::chat_manager::capture_active_request_epochs(
        &active_requests,
        &topic_key,
    );
    let mutation = crate::vcp_modules::message_service::truncate_history_after_timestamp_for_topic(
        &db_state.pool,
        &topic_key,
        &anchor_message_id,
        false,
    )
    .await?;
    crate::vcp_modules::chat_manager::cancel_captured_active_requests(
        &active_requests,
        captured,
        &mutation.active_ids,
    )
    .await;
    let settings =
        crate::vcp_modules::settings_manager::read_settings(app_handle.clone(), settings_state)
            .await?;
    let dispatch = dispatch_regeneration(
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
    .await?;
    Ok(json!({
        "status": "started",
        "msgCount": mutation.msg_count,
        "deletedIds": mutation.deleted_ids,
        "anchor": mutation.anchor,
        "dispatch": dispatch,
    }))
}

async fn load_regeneration_message(
    pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    message_id: &str,
) -> Result<(ChatMessage, String), String> {
    let row = sqlx::query(
        "SELECT msg_id, timestamp, role, name, content, updated_at, agent_id, group_id,
                is_group_message, finish_reason
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&topic_key.owner_type)
    .bind(&topic_key.owner_id)
    .bind(&topic_key.topic_id)
    .bind(message_id)
    .fetch_one(pool)
    .await
    .map_err(|error| format!("目标响应不存在、已删除或不属于当前话题：{error}"))?;
    let target_timestamp: i64 = row
        .try_get("timestamp")
        .map_err(|error| format!("读取目标响应时间失败：{error}"))?;
    let target_role: String = row
        .try_get("role")
        .map_err(|error| format!("读取目标响应角色失败：{error}"))?;
    let (anchor_id, anchor_row) = if target_role == "user" {
        (message_id.to_string(), row)
    } else {
        let anchor_row = sqlx::query(
            "SELECT msg_id, timestamp, role, name, content, updated_at, agent_id, group_id,
                    is_group_message, finish_reason
             FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND deleted_at IS NULL AND role = 'user'
               AND (timestamp < ? OR (timestamp = ? AND msg_id < ?))
             ORDER BY timestamp DESC, msg_id DESC LIMIT 1",
        )
        .bind(&topic_key.owner_type)
        .bind(&topic_key.owner_id)
        .bind(&topic_key.topic_id)
        .bind(target_timestamp)
        .bind(target_timestamp)
        .bind(message_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("查找完整历史中的 user 锚点失败：{error}"))?
        .ok_or_else(|| "目标响应之前不存在可重新生成的 user 消息".to_string())?;
        let anchor_id: String = anchor_row
            .try_get("msg_id")
            .map_err(|error| format!("读取 user 锚点 ID 失败：{error}"))?;
        (anchor_id, anchor_row)
    };
    let message = decode_regeneration_message(pool, topic_key, anchor_row).await?;
    Ok((message, anchor_id))
}

async fn decode_regeneration_message(
    _pool: &sqlx::SqlitePool,
    topic_key: &TopicKey,
    row: sqlx::sqlite::SqliteRow,
) -> Result<ChatMessage, String> {
    let content = crate::vcp_modules::persistence::message_content_storage::decode_message_content(
        &row, "content",
    )?;
    let timestamp: i64 = row
        .try_get("timestamp")
        .map_err(|error| format!("读取 user 锚点时间失败：{error}"))?;
    let updated_at: i64 = row
        .try_get("updated_at")
        .map_err(|error| format!("读取 user 锚点更新时间失败：{error}"))?;
    Ok(ChatMessage {
        id: row
            .try_get("msg_id")
            .map_err(|error| format!("读取 user 锚点 ID 失败：{error}"))?,
        role: row
            .try_get("role")
            .map_err(|error| format!("读取 user 锚点角色失败：{error}"))?,
        name: row
            .try_get("name")
            .map_err(|error| format!("读取 user 锚点名称失败：{error}"))?,
        content,
        timestamp: u64::try_from(timestamp).map_err(|_| "user 锚点时间无效".to_string())?,
        updated_at: Some(
            u64::try_from(updated_at).map_err(|_| "user 锚点更新时间无效".to_string())?,
        ),
        is_thinking: Some(false),
        agent_id: row
            .try_get("agent_id")
            .map_err(|error| format!("读取 user 锚点 agent 失败：{error}"))?,
        group_id: row
            .try_get("group_id")
            .map_err(|error| format!("读取 user 锚点 group 失败：{error}"))?,
        topic_id: Some(topic_key.topic_id.clone()),
        is_group_message: Some(
            row.try_get::<i64, _>("is_group_message")
                .map_err(|error| format!("读取 user 锚点类型失败：{error}"))?
                != 0,
        ),
        finish_reason: row
            .try_get("finish_reason")
            .map_err(|error| format!("读取 user 锚点结束原因失败：{error}"))?,
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    })
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
