use super::message_service_support::{resolve_unique_topic_key, topic_key};
use crate::vcp_modules::topic_types::TopicKey;
use serde::{Deserialize, Serialize};
use sqlx::Sqlite;
use std::collections::HashSet;
use tauri::AppHandle;

#[path = "message_service_attachments_delete.rs"]
mod attachments_delete;
#[path = "message_service_deletion_queries.rs"]
mod deletion_queries;
#[path = "message_service_truncation.rs"]
mod truncation;

pub use attachments_delete::{delete_message_attachment, delete_message_attachment_for_key};
use deletion_queries::{
    delete_message_related_rows, mark_message_ids_deleted, placeholders, select_active_message_ids,
    select_live_message_ids,
};
pub(crate) use truncation::{
    bubble_topic_hash, delete_ordered_related_rows, mark_ordered_messages_deleted, order_predicate,
    qualified_order_predicate, refresh_topic_message_count, select_ordered_active_ids,
    select_ordered_message_ids,
};

/// 消息删除/截断在同一事务中产生的权威结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageMutationResult {
    pub deleted_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub deleted_at: i64,
    pub msg_count: i32,
    pub anchor: Option<MessageMutationAnchor>,
}

/// 记录本次截断使用的稳定锚点，供分页历史在提交后收敛。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageMutationAnchor {
    pub message_id: String,
    pub timestamp: i64,
    pub include_anchor: bool,
}

/// 兼容仅持有话题 ID 的调用方；跨 owner 的重复话题 ID 会安全失败。
pub async fn delete_messages(
    db_pool: &sqlx::Pool<Sqlite>,
    topic_id: &str,
    msg_ids: Vec<String>,
) -> Result<MessageMutationResult, String> {
    let key = resolve_unique_topic_key(db_pool, topic_id).await?;
    delete_messages_for_topic(db_pool, &key, msg_ids).await
}

/// 按 owner 执行逻辑删除，在同一事务提交前更新附属数据和话题哈希。
pub async fn delete_messages_for_topic(
    db_pool: &sqlx::Pool<Sqlite>,
    key: &TopicKey,
    msg_ids: Vec<String>,
) -> Result<MessageMutationResult, String> {
    validate_message_ids(key, &msg_ids)?;
    let mut tx = db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|error| error.to_string())?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    let placeholders = placeholders(msg_ids.len());
    let deleted_ids = if msg_ids.is_empty() {
        Vec::new()
    } else {
        select_live_message_ids(&mut tx, key, &msg_ids, &placeholders).await?
    };
    let active_ids = if msg_ids.is_empty() {
        Vec::new()
    } else {
        select_active_message_ids(&mut tx, key, &msg_ids, &placeholders).await?
    };
    if !msg_ids.is_empty() {
        mark_message_ids_deleted(&mut tx, key, &msg_ids, &placeholders, now).await?;
        delete_message_related_rows(&mut tx, key, &msg_ids, &placeholders).await?;
    }
    let msg_count = refresh_topic_message_count(&mut tx, key, now).await?;
    bubble_topic_hash(&mut tx, key).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(MessageMutationResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count,
        anchor: None,
    })
}

/// 保留历史 Tauri 命令符号，但使用稳定消息锚点替代有歧义的时间戳。
pub async fn truncate_history_after_timestamp(
    _app_handle: AppHandle,
    db_pool: &sqlx::Pool<Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    anchor_message_id: &str,
    include_anchor: bool,
) -> Result<MessageMutationResult, String> {
    let key = topic_key(owner_id, owner_type, topic_id)?;
    truncate_history_after_timestamp_for_topic(db_pool, &key, anchor_message_id, include_anchor)
        .await
}

/// 删除精确消息身份之后，或从精确消息身份开始的历史。
/// 全序为 `(timestamp, msg_id)`，因此同毫秒消息不会被误保留或误删。
pub async fn truncate_history_after_timestamp_for_topic(
    db_pool: &sqlx::Pool<Sqlite>,
    key: &TopicKey,
    anchor_message_id: &str,
    include_anchor: bool,
) -> Result<MessageMutationResult, String> {
    truncation::truncate_history_after_timestamp_for_topic(
        db_pool,
        key,
        anchor_message_id,
        include_anchor,
    )
    .await
}

fn validate_message_ids(key: &TopicKey, msg_ids: &[String]) -> Result<(), String> {
    if !key.is_valid() {
        return Err("话题身份不完整".to_string());
    }
    if msg_ids.len() > 10_000
        || msg_ids.iter().any(String::is_empty)
        || msg_ids.iter().collect::<HashSet<_>>().len() != msg_ids.len()
    {
        return Err("消息删除需要 1..=10000 个不重复的消息 ID".to_string());
    }
    Ok(())
}
