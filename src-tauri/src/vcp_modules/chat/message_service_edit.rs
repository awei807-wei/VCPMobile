use super::super::message_service_deletions::{
    bubble_topic_hash, delete_ordered_related_rows, mark_ordered_messages_deleted,
    qualified_order_predicate, refresh_topic_message_count, select_ordered_active_ids,
    select_ordered_message_ids, MessageMutationAnchor,
};
use super::EditMessageMutationResult;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::infra::file_manager::AttachmentReadGuard;
use crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, MessageRepository};
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};

pub(super) async fn edit_message_and_truncate_history_with_loaded_attachments_with_gate(
    db_pool: &sqlx::Pool<Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    anchor_message_id: String,
    mut message: ChatMessage,
    roots: Option<&ManagedAttachmentRoots>,
    gate: &AttachmentReadGuard,
) -> Result<EditMessageMutationResult, String> {
    let key = super::super::message_service_support::topic_key(owner_id, owner_type, &topic_id)?;
    validate_edit_identity(&key, &anchor_message_id, &message)?;
    let blocks = compile_message_blocks(&message)?;
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let mut tx = db_pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|error| error.to_string())?;
    let anchor = load_edit_anchor(&mut tx, &key, &anchor_message_id).await?;
    apply_anchor_identity(&mut message, &key, &anchor)?;
    MessageRepository::upsert_message_for_topic_with_attachment_gate_and_roots(
        &mut tx,
        &message,
        &key,
        &render_bytes,
        true,
        gate,
        roots,
    )
    .await?;
    let predicate = qualified_order_predicate(false);
    let (deleted_ids, active_ids, now, msg_count) = truncate_edited_tail(
        &mut tx,
        &key,
        anchor.timestamp,
        &anchor_message_id,
        predicate,
    )
    .await?;
    tx.commit().await.map_err(|error| error.to_string())?;

    Ok(EditMessageMutationResult {
        deleted_ids,
        active_ids,
        deleted_at: now,
        msg_count,
        anchor: Some(MessageMutationAnchor {
            message_id: anchor_message_id,
            timestamp: anchor.timestamp,
            include_anchor: false,
        }),
        blocks,
    })
}

async fn truncate_edited_tail(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(Vec<String>, Vec<String>, i64, i32), String> {
    let (deleted_ids, active_ids) =
        collect_deleted_ids(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    delete_ordered_related_rows(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    let now = crate::vcp_modules::infra::utils::now_millis();
    mark_ordered_messages_deleted(
        tx,
        key,
        anchor_timestamp,
        anchor_message_id,
        super::super::message_service_deletions::order_predicate(false),
        now,
        deleted_ids.len(),
    )
    .await?;
    let msg_count = refresh_topic_message_count(tx, key, now).await?;
    // Hash bubbling is deliberately last: any cache/index/tail/count/hash error
    // drops the transaction and restores the exact pre-edit history.
    bubble_topic_hash(tx, key).await?;
    Ok((deleted_ids, active_ids, now, msg_count))
}

fn validate_edit_identity(
    key: &TopicKey,
    anchor_message_id: &str,
    message: &ChatMessage,
) -> Result<(), String> {
    if anchor_message_id.is_empty() || message.id != anchor_message_id {
        return Err("编辑重发需要与 anchorMessageId 一致的消息身份".to_string());
    }
    if message
        .topic_id
        .as_deref()
        .is_some_and(|message_topic_id| message_topic_id != key.topic_id)
    {
        return Err("编辑重发消息的 topicId 与目标话题不一致".to_string());
    }
    Ok(())
}

fn compile_message_blocks(
    message: &ChatMessage,
) -> Result<Vec<crate::vcp_modules::content_parser::ContentBlock>, String> {
    if let Some(blocks) = &message.blocks {
        serde_json::from_value(blocks.clone()).map_err(|error| error.to_string())
    } else {
        Ok(MessageRenderCompiler::compile(&message.content))
    }
}

fn apply_anchor_identity(
    message: &mut ChatMessage,
    key: &TopicKey,
    anchor: &EditAnchor,
) -> Result<(), String> {
    message.timestamp =
        u64::try_from(anchor.timestamp).map_err(|_| "编辑锚点时间无效".to_string())?;
    // 编辑重发只允许替换正文；消息身份、角色和所属话题均以数据库锚点为准。
    message.role = anchor.role.clone();
    message.name = anchor.name.clone();
    message.agent_id = anchor.agent_id.clone();
    message.group_id = anchor.group_id.clone();
    message.is_group_message = Some(anchor.is_group_message);
    message.finish_reason = anchor.finish_reason.clone();
    message.topic_id = Some(key.topic_id.clone());
    message.updated_at = None;
    Ok(())
}

async fn collect_deleted_ids(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    anchor_timestamp: i64,
    anchor_message_id: &str,
    predicate: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let deleted_ids =
        select_ordered_message_ids(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    let active_ids =
        select_ordered_active_ids(tx, key, anchor_timestamp, anchor_message_id, predicate).await?;
    Ok((deleted_ids, active_ids))
}

struct EditAnchor {
    timestamp: i64,
    role: String,
    name: Option<String>,
    agent_id: Option<String>,
    group_id: Option<String>,
    is_group_message: bool,
    finish_reason: Option<String>,
}

async fn load_edit_anchor(
    tx: &mut Transaction<'_, Sqlite>,
    key: &TopicKey,
    message_id: &str,
) -> Result<EditAnchor, String> {
    if !key.is_valid() || message_id.is_empty() {
        return Err("编辑重发需要完整话题身份和 anchorMessageId".to_string());
    }
    let row = sqlx::query(
        "SELECT timestamp, role, name, agent_id, group_id,
                is_group_message, finish_reason
         FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("anchorMessageId {message_id} 不存在、已删除或不属于当前话题"))?;
    Ok(EditAnchor {
        timestamp: row
            .try_get("timestamp")
            .map_err(|error| error.to_string())?,
        role: row.try_get("role").map_err(|error| error.to_string())?,
        name: row.try_get("name").map_err(|error| error.to_string())?,
        agent_id: row.try_get("agent_id").map_err(|error| error.to_string())?,
        group_id: row.try_get("group_id").map_err(|error| error.to_string())?,
        is_group_message: row
            .try_get::<i64, _>("is_group_message")
            .map_err(|error| error.to_string())?
            != 0,
        finish_reason: row
            .try_get("finish_reason")
            .map_err(|error| error.to_string())?,
    })
}
