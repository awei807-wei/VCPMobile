use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::infra::vcp_client::ActiveRequestRegistry;
use crate::vcp_modules::message_service;
use crate::vcp_modules::topic_types::TopicKey;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Attachment {
    #[serde(default)]
    pub r#type: String,
    /// 物理存储路径：真理之源。用于后续超栈文件追踪，或跨端同步时的原始路径参考
    #[serde(default)]
    pub src: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(
        rename = "attachmentOrder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub attachment_order: Option<i32>,

    // 平铺数据库中的核心附件字段
    #[serde(rename = "internalPath", default)]
    pub internal_path: String,
    #[serde(rename = "extractedText", skip_serializing_if = "Option::is_none")]
    pub extracted_text: Option<String>,
    #[serde(rename = "imageFrames", skip_serializing_if = "Option::is_none")]
    pub image_frames: Option<Vec<String>>,
    #[serde(rename = "thumbnailPath", skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    #[serde(alias = "senderName")]
    pub name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isThinking")]
    #[serde(default)]
    pub is_thinking: Option<bool>,

    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "groupId", skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "topicId", skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    #[serde(rename = "isGroupMessage", skip_serializing_if = "Option::is_none")]
    pub is_group_message: Option<bool>,
    #[serde(rename = "finishReason", skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,

    #[serde(default)]
    pub attachments: Option<Vec<Attachment>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<crate::vcp_modules::pre_renderer::MessageShell>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryChunk {
    pub message: ChatMessage,
    pub index: usize,
    pub is_last: bool,
}

// --- 历史记录存取逻辑 ---

#[tauri::command]
pub async fn load_chat_history_streamed(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
    on_message: tauri::ipc::Channel<HistoryChunk>,
) -> Result<usize, String> {
    let messages = crate::vcp_modules::message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        limit,
        offset,
        false,
        false, // include_extracted_text: 前端列表加载不需要大体积的提取文本内容
    )
    .await?;

    let total = messages.len();
    for (index, message) in messages.into_iter().enumerate() {
        let is_last = index == total.saturating_sub(1);
        let _ = on_message.send(HistoryChunk {
            message,
            index,
            is_last,
        });
    }
    Ok(total)
}

#[tauri::command]
pub async fn load_chat_history(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ChatMessage>, String> {
    crate::vcp_modules::message_service::load_chat_history_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        limit,
        offset,
        false,
        false,
    )
    .await
}

/// 以完整复合身份加载全局搜索目标消息附近的历史窗口。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn load_chat_history_around(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    anchor_message_id: String,
    before_count: Option<usize>,
    after_count: Option<usize>,
) -> Result<message_service::HistoryAroundResult, String> {
    message_service::load_chat_history_around_internal(
        &app_handle,
        &owner_id,
        &owner_type,
        &topic_id,
        &anchor_message_id,
        before_count,
        after_count,
    )
    .await
}

#[tauri::command]
pub async fn append_single_message(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    message_service::append_single_message(
        app_handle,
        &db_state.pool,
        &owner_id,
        &owner_type,
        topic_id,
        message,
    )
    .await
}

/// 在单一 SQLite mutation 中编辑锚点并截断其后的历史。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn edit_message_and_truncate_history(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    active_requests: tauri::State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    anchor_message_id: String,
    message: ChatMessage,
) -> Result<message_service::EditMessageMutationResult, String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    let active_registry = active_requests.0.clone();
    let mutation_topic = topic_key.clone();
    let operation_registry = active_registry.clone();
    let pool = db_state.pool.clone();
    active_registry
        .with_topic_mutation(&topic_key, move |captured| {
            let active_registry = operation_registry;
            let topic_key = mutation_topic;
            async move {
                let result = message_service::edit_message_and_truncate_history(
                    app_handle,
                    &pool,
                    &topic_key.owner_id,
                    &topic_key.owner_type,
                    topic_key.topic_id.clone(),
                    anchor_message_id,
                    message,
                )
                .await?;
                cancel_captured_active_request_epochs_all(&active_registry, captured).await;
                Ok(result)
            }
        })
        .await
}

#[tauri::command]
pub async fn patch_single_message(
    app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    message_service::patch_single_message(
        app_handle,
        &db_state.pool,
        &owner_id,
        &owner_type,
        topic_id,
        message,
        false,
    )
    .await
}

#[tauri::command]
pub async fn delete_messages(
    _app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    active_requests: tauri::State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    msg_ids: Vec<String>,
) -> Result<message_service::MessageMutationResult, String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    let captured = capture_active_request_epochs(&active_requests, &topic_key);
    let result =
        message_service::delete_messages_for_topic(&db_state.pool, &topic_key, msg_ids).await?;
    cancel_captured_active_requests(&active_requests, captured, &result.active_ids).await;
    Ok(result)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri IPC 要求每个命令字段保持独立参数。
pub async fn truncate_history_after_timestamp(
    _app_handle: tauri::AppHandle,
    db_state: tauri::State<'_, crate::vcp_modules::db_manager::DbState>,
    active_requests: tauri::State<'_, crate::vcp_modules::vcp_client::ActiveRequests>,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    anchor_message_id: String,
    include_anchor: bool,
) -> Result<message_service::MessageMutationResult, String> {
    let topic_key = TopicKey::new(owner_type, owner_id, topic_id);
    let captured = capture_active_request_epochs(&active_requests, &topic_key);
    let result = message_service::truncate_history_after_timestamp_for_topic(
        &db_state.pool,
        &topic_key,
        &anchor_message_id,
        include_anchor,
    )
    .await?;
    cancel_captured_active_requests(&active_requests, captured, &result.active_ids).await;
    Ok(result)
}

/// 在 mutation 开始前捕获本话题的完整请求身份和 registry epoch。
pub(crate) fn capture_active_request_epochs(
    active_requests: &crate::vcp_modules::vcp_client::ActiveRequests,
    topic_key: &TopicKey,
) -> Vec<(crate::vcp_modules::topic_types::MessageKey, u64)> {
    active_requests.0.snapshot_epochs_for_topic(topic_key)
}

/// 只取消 mutation 开始前捕获、且仍属于相同 epoch 的旧请求。
pub(crate) async fn cancel_captured_active_requests(
    active_requests: &crate::vcp_modules::vcp_client::ActiveRequests,
    captured: Vec<(crate::vcp_modules::topic_types::MessageKey, u64)>,
    affected_ids: &[String],
) {
    cancel_captured_active_request_epochs(&active_requests.0, captured, affected_ids).await;
}

pub(crate) async fn cancel_captured_active_request_epochs(
    active_requests: &ActiveRequestRegistry,
    captured: Vec<(crate::vcp_modules::topic_types::MessageKey, u64)>,
    affected_ids: &[String],
) {
    let affected = affected_ids.iter().collect::<HashSet<_>>();
    for (key, epoch) in captured {
        if !affected.contains(&key.msg_id) {
            continue;
        }
        let _ = active_requests.cancel_if_current(&key, epoch).await;
    }
}

pub(crate) async fn cancel_captured_active_request_epochs_all(
    active_requests: &ActiveRequestRegistry,
    captured: Vec<(crate::vcp_modules::topic_types::MessageKey, u64)>,
) {
    for (key, epoch) in captured {
        let _ = active_requests.cancel_if_current(&key, epoch).await;
    }
}

// --- 增量同步逻辑 (Delta Sync) (Moved to sync_manager.rs) ---
