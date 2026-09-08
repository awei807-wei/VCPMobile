use super::message_service_mutations::{
    append_single_message, patch_single_message, patch_single_message_with_loaded_attachments,
};
use super::message_service_support::load_attachments_for_topic;
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::infra::vcp_client::{CompletionLease, GuardedTransition};
use crate::vcp_modules::topic_types::MessageKey;
use sqlx::{sqlite::SqliteRow, Row};
use tauri::{ipc::Channel, AppHandle, Runtime};

#[path = "message_service_stream_events.rs"]
mod events;
use events::send_stream_end;

#[path = "message_service_stream_finalize.rs"]
mod finalize;

/// 流式终结结果；旧请求被替换时返回 `Skipped`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFinalizationStatus {
    Applied,
    Skipped,
}

/// 在请求租约内创建助手骨架，并由现有仓储维护哈希、缓存和全文索引。
pub async fn persist_stream_skeleton_guarded<R: Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    lease: &CompletionLease,
    agent_id: Option<String>,
    name: Option<String>,
) -> Result<StreamFinalizationStatus, String> {
    let pool = pool.clone();
    let transition = lease
        .with_current_transition(|key| async move {
            let is_group = key.topic.owner_type == "group";
            let timestamp = crate::vcp_modules::infra::utils::now_millis() as u64;
            let message = ChatMessage {
                id: key.msg_id.clone(),
                role: "assistant".to_string(),
                name,
                content: String::new(),
                timestamp,
                updated_at: Some(timestamp),
                is_thinking: Some(true),
                agent_id: if is_group {
                    agent_id
                } else {
                    Some(key.topic.owner_id.clone())
                },
                group_id: is_group.then(|| key.topic.owner_id.clone()),
                topic_id: Some(key.topic.topic_id.clone()),
                is_group_message: Some(is_group),
                finish_reason: None,
                ..Default::default()
            };
            append_single_message(
                app_handle,
                &pool,
                &key.topic.owner_id,
                &key.topic.owner_type,
                key.topic.topic_id,
                message,
            )
            .await
            .map(|_| ())
        })
        .await?;
    Ok(match transition {
        GuardedTransition::Applied(()) => StreamFinalizationStatus::Applied,
        GuardedTransition::Skipped => StreamFinalizationStatus::Skipped,
    })
}

/// 用现有消息仓储更新正文，统一维护正文压缩、content_hash、render_cache 与 FTS。
pub async fn update_existing_message_content<R: Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    content: String,
    finish_reason: Option<String>,
    is_thinking: bool,
) -> Result<Vec<ContentBlock>, String> {
    let message =
        load_content_update_message(pool, key, content, finish_reason, is_thinking).await?;
    patch_single_message(
        app_handle,
        pool,
        &key.topic.owner_id,
        &key.topic.owner_type,
        key.topic.topic_id.clone(),
        message,
        false,
    )
    .await
}

/// 使用数据库中已装载的附件执行正文更新，供恢复和确定性测试复用生产路径。
pub async fn update_existing_message_content_for_pool(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    content: String,
    finish_reason: Option<String>,
    is_thinking: bool,
) -> Result<Vec<ContentBlock>, String> {
    let message =
        load_content_update_message(pool, key, content, finish_reason, is_thinking).await?;
    patch_single_message_with_loaded_attachments(
        pool,
        &key.topic.owner_id,
        &key.topic.owner_type,
        key.topic.topic_id.clone(),
        message,
        false,
    )
    .await
}

async fn load_content_update_message(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    content: String,
    finish_reason: Option<String>,
    is_thinking: bool,
) -> Result<ChatMessage, String> {
    let row = sqlx::query(
        "SELECT role, name, agent_id, content, timestamp, updated_at,
                is_group_message, group_id, finish_reason
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("读取待更新消息失败: {error}"))?
    .ok_or_else(|| format!("消息 {}/{} 不存在", key.topic.topic_id, key.msg_id))?;
    let attachment_map =
        load_attachments_for_topic(pool, &key.topic, std::slice::from_ref(&key.msg_id), true)
            .await?;
    build_content_update_message(
        &row,
        key,
        content,
        finish_reason,
        is_thinking,
        attachment_map.get(&key.msg_id).cloned(),
    )
}

fn build_content_update_message(
    row: &SqliteRow,
    key: &MessageKey,
    content: String,
    finish_reason: Option<String>,
    is_thinking: bool,
    attachments: Option<Vec<Attachment>>,
) -> Result<ChatMessage, String> {
    let timestamp: i64 = row
        .try_get("timestamp")
        .map_err(|error| format!("读取消息时间失败: {error}"))?;
    let updated_at: i64 = row
        .try_get("updated_at")
        .map_err(|error| format!("读取消息更新时间失败: {error}"))?;
    let existing_finish_reason: Option<String> = row
        .try_get("finish_reason")
        .map_err(|error| format!("读取消息结束原因失败: {error}"))?;
    Ok(ChatMessage {
        id: key.msg_id.clone(),
        role: row
            .try_get("role")
            .map_err(|error| format!("读取消息角色失败: {error}"))?,
        name: row
            .try_get("name")
            .map_err(|error| format!("读取消息名称失败: {error}"))?,
        content,
        timestamp: u64::try_from(timestamp).map_err(|_| "消息时间戳为负数".to_string())?,
        updated_at: Some(u64::try_from(updated_at).map_err(|_| "消息更新时间为负数".to_string())?),
        is_thinking: Some(is_thinking),
        agent_id: row
            .try_get("agent_id")
            .map_err(|error| format!("读取消息代理失败: {error}"))?,
        group_id: row
            .try_get("group_id")
            .map_err(|error| format!("读取消息群组失败: {error}"))?,
        topic_id: Some(key.topic.topic_id.clone()),
        is_group_message: Some(
            row.try_get::<i64, _>("is_group_message")
                .map_err(|error| format!("读取消息类型失败: {error}"))?
                != 0,
        ),
        finish_reason: finish_reason.or(existing_finish_reason),
        attachments,
        blocks: None,
        shell: None,
        content_hash: None,
    })
}

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
    generation: Option<u64>,
) -> Result<(), String> {
    let key = MessageKey::new(
        crate::vcp_modules::topic_types::TopicKey::new(owner_type, owner_id, &topic_id),
        &message_id,
    );
    if !key.is_valid() {
        return Err("流式消息终结缺少完整身份".to_string());
    }
    finalize_stream_message_inner(
        app_handle,
        pool,
        &key,
        full_content,
        is_aborted,
        finish_reason,
        stream_channel,
        agent_id,
        generation,
    )
    .await
}

/// 使用请求完成租约执行持久化与 end 事件，旧 epoch 会明确返回 `Skipped`。
#[allow(clippy::too_many_arguments)]
pub async fn finalize_stream_message_guarded<R: Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    lease: &CompletionLease,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
) -> Result<StreamFinalizationStatus, String> {
    let pool = pool.clone();
    let transition = lease
        .with_current_transition(|key| async move {
            finalize_stream_message_inner(
                app_handle,
                &pool,
                &key,
                full_content,
                is_aborted,
                finish_reason,
                stream_channel,
                agent_id,
                Some(lease.epoch()),
            )
            .await
        })
        .await?;
    Ok(match transition {
        GuardedTransition::Applied(()) => StreamFinalizationStatus::Applied,
        GuardedTransition::Skipped => StreamFinalizationStatus::Skipped,
    })
}

#[allow(clippy::too_many_arguments)]
async fn finalize_stream_message_inner<R: Runtime>(
    app_handle: AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    full_content: String,
    is_aborted: bool,
    finish_reason: Option<String>,
    stream_channel: Option<Channel<crate::vcp_modules::vcp_client::StreamEvent>>,
    agent_id: Option<String>,
    generation: Option<u64>,
) -> Result<(), String> {
    let final_ts = crate::vcp_modules::infra::utils::now_millis() as u64;
    let is_group = key.topic.owner_type == "group";
    let final_msg = finalize::build_final_message(
        pool,
        &key.topic.owner_id,
        &key.topic.topic_id,
        key.msg_id.clone(),
        full_content,
        is_aborted,
        finish_reason.clone(),
        agent_id,
        is_group,
        final_ts,
    )
    .await?;
    let end_blocks = finalize::persist_final_message(
        &app_handle,
        pool,
        &key.topic.owner_id,
        &key.topic.topic_id,
        final_msg,
        is_group,
    )
    .await?;
    finalize::clear_active_generation(pool, key).await?;
    send_stream_end(
        stream_channel,
        key.msg_id.clone(),
        &key.topic.owner_id,
        &key.topic.topic_id,
        is_group,
        finish_reason,
        Some(end_blocks),
        final_ts,
        generation,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "message_service_stream_tests.rs"]
mod tests;
