use super::super::message_service_mutations::{append_single_message, patch_single_message};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::topic_types::MessageKey;
use sqlx::Row;
use tauri::{AppHandle, Runtime};

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_final_message(
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
) -> Result<ChatMessage, String> {
    let mut content = full_content;
    if is_aborted {
        content.push_str("\n\n> VCP流式错误: 请求已中止");
    }
    let final_agent_id = if is_group {
        agent_id
    } else {
        Some(owner_id.to_string())
    };
    let name = load_agent_name(pool, final_agent_id.as_deref()).await?;
    Ok(ChatMessage {
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
    })
}

async fn load_agent_name(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    agent_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(agent_id) = agent_id else {
        return Ok(None);
    };
    sqlx::query("SELECT name FROM agents WHERE agent_id = ?")
        .bind(agent_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("读取代理名称失败: {error}"))?
        .map(|row| {
            row.try_get("name")
                .map_err(|error| format!("读取代理名称字段失败: {error}"))
        })
        .transpose()
}

pub(super) async fn persist_final_message<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    topic_id: &str,
    message: ChatMessage,
    is_group: bool,
) -> Result<Vec<ContentBlock>, String> {
    if owner_id.is_empty() || topic_id.is_empty() {
        return Err("流式消息终结缺少 ownerId 或 topicId".to_string());
    }
    let result: Result<Vec<ContentBlock>, String> = if is_group {
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
    result.map_err(|error| {
        let operation = if is_group { "追加" } else { "更新" };
        format!("流式消息终结{operation}最终正文失败: {error}")
    })
}

pub(super) async fn clear_active_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<(), String> {
    sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|error| format!("清理 active_generations 失败: {error}"))
}
