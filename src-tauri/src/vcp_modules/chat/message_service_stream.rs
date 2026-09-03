use super::message_service_mutations::{append_single_message, patch_single_message};
use crate::vcp_modules::chat_manager::ChatMessage;
use tauri::ipc::Channel;

#[allow(clippy::too_many_arguments)]
pub async fn finalize_stream_message<R: tauri::Runtime>(
    app_handle: tauri::AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    message_id: String,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
) -> Result<(), String> {
    let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;
    let is_group = owner_type == "group";
    let final_msg = build_final_message(
        pool,
        owner_id,
        &topic_id,
        message_id.clone(),
        full_content,
        is_aborted,
        finish_reason.clone(),
        agent_id,
        is_group,
        final_ts,
    )
    .await;
    let end_blocks =
        persist_final_message(&app_handle, pool, owner_id, &topic_id, final_msg, is_group).await;
    clear_active_generation(pool, owner_type, owner_id, &topic_id, &message_id).await;
    send_stream_end(
        stream_channel,
        message_id,
        owner_id,
        &topic_id,
        is_group,
        finish_reason,
        end_blocks,
        final_ts,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn build_final_message(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    topic_id: &str,
    message_id: String,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    agent_id: Option<String>,
    is_group: bool,
    final_ts: u64,
) -> ChatMessage {
    let mut content = full_content;
    if is_aborted {
        content.push_str("\n\n> VCP流式错误: 请求已中止");
    }
    let final_agent_id = if is_group {
        agent_id
    } else {
        Some(owner_id.to_string())
    };
    let name = load_agent_name(pool, final_agent_id.as_deref()).await;
    ChatMessage {
        id: message_id,
        role: "assistant".to_string(),
        name,
        content,
        timestamp: final_ts,
        updated_at: Some(final_ts),
        is_thinking: Some(false),
        agent_id: final_agent_id,
        group_id: is_group.then(|| owner_id.to_string()),
        topic_id: Some(topic_id.to_string()),
        is_group_message: Some(is_group),
        finish_reason,
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    }
}

async fn load_agent_name(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: Option<&str>,
) -> Option<String> {
    let agent_id = agent_id?;
    sqlx::query("SELECT name FROM agents WHERE agent_id = ?")
        .bind(agent_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|row| sqlx::Row::try_get(&row, "name").ok())
}

async fn persist_final_message<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    topic_id: &str,
    message: ChatMessage,
    is_group: bool,
) -> Option<Vec<crate::vcp_modules::content_parser::ContentBlock>> {
    if owner_id.is_empty() || topic_id.is_empty() {
        return None;
    }
    let result = if is_group {
        append_single_message(
            app_handle.clone(),
            pool,
            owner_id,
            "group",
            topic_id.to_string(),
            message,
        )
        .await
    } else {
        patch_single_message(
            app_handle.clone(),
            pool,
            owner_id,
            "agent",
            topic_id.to_string(),
            message,
            false,
        )
        .await
    };
    result.map(Some).unwrap_or_else(|error| {
        let operation = if is_group { "append" } else { "patch" };
        log::error!("[StreamFinalizer] Failed to {operation} final message: {error}");
        None
    })
}

async fn clear_active_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    message_id: &str,
) {
    if message_id.is_empty() {
        return;
    }
    let _ = sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(topic_id)
    .bind(message_id)
    .execute(pool)
    .await;
}

#[allow(clippy::too_many_arguments)]
fn send_stream_end(
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    message_id: String,
    owner_id: &str,
    topic_id: &str,
    is_group: bool,
    finish_reason: Option<String>,
    end_blocks: Option<Vec<crate::vcp_modules::content_parser::ContentBlock>>,
    final_ts: u64,
) {
    let Some(channel) = stream_channel else {
        return;
    };
    let context = build_stream_context(owner_id, topic_id, is_group);
    let _ = channel.send(crate::vcp_modules::vcp_client::StreamEvent::end(
        message_id,
        context,
        Some(finish_reason.unwrap_or_else(|| "completed".to_string())),
        end_blocks,
        Some(final_ts),
    ));
}

fn build_stream_context(
    owner_id: &str,
    topic_id: &str,
    is_group: bool,
) -> Option<serde_json::Value> {
    if owner_id.is_empty() || topic_id.is_empty() {
        return None;
    }
    if is_group {
        Some(serde_json::json!({
            "groupId": owner_id,
            "topicId": topic_id,
            "isGroupMessage": true,
        }))
    } else {
        Some(serde_json::json!({
            "agentId": owner_id,
            "topicId": topic_id,
        }))
    }
}
