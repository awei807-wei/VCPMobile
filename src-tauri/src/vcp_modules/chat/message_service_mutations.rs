use super::message_service_support::{ensure_attachments_locally, topic_key};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::content_parser::ContentBlock;
use crate::vcp_modules::infra::file_manager::attachment_gc_gate;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, MessageRepository};
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::{AppHandle, Manager};

#[path = "message_service_edit.rs"]
mod edit;

/// 编辑重发在单一 SQLite 事务内返回的权威结果。
///
/// `deleted_ids`、`msg_count` 和 `anchor` 与截断 mutation 保持相同语义，
/// `blocks` 是已提交锚点正文对应的渲染结果，供前端在一次提交后收敛。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditMessageMutationResult {
    pub deleted_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub deleted_at: i64,
    pub msg_count: i32,
    pub anchor: Option<super::message_service_deletions::MessageMutationAnchor>,
    pub blocks: Vec<ContentBlock>,
}

pub async fn append_single_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
) -> Result<Vec<ContentBlock>, String> {
    let gate = attachment_gc_gate().read().await;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(&app_handle)?;
    ensure_attachments_locally(&app_handle, &mut message).await?;
    let key = topic_key(owner_id, owner_type, &topic_id)?;
    let blocks = if let Some(blocks) = &message.blocks {
        serde_json::from_value(blocks.clone()).map_err(|error| error.to_string())?
    } else {
        MessageRenderCompiler::compile(&message.content)
    };
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let mut tx = db_pool.begin().await.map_err(|error| error.to_string())?;
    MessageRepository::upsert_message_for_topic_with_attachment_gate_and_roots(
        &mut tx,
        &message,
        &key,
        &render_bytes,
        false,
        &gate,
        Some(&roots),
    )
    .await?;
    if message.role == "assistant" && message.finish_reason.is_none() {
        register_active_generation(&mut tx, &key, &message).await?;
    }
    refresh_message_topic(&mut tx, &key, &message).await?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(blocks)
}

/// 编辑 user 锚点并删除其后的历史；正文、缓存、索引、附件关系、计数和
/// 同步哈希都在同一 SQLite 事务中完成。
pub async fn edit_message_and_truncate_history<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    anchor_message_id: String,
    mut message: ChatMessage,
) -> Result<EditMessageMutationResult, String> {
    let gate = attachment_gc_gate().read().await;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(&app_handle)?;
    ensure_attachments_locally(&app_handle, &mut message).await?;
    let key = topic_key(owner_id, owner_type, &topic_id)?;
    edit_message_and_truncate_history_with_loaded_attachments_with_gate(
        db_pool,
        &key,
        anchor_message_id,
        message,
        Some(&roots),
        &gate,
    )
    .await
}

/// 与生产编辑重发路径相同的事务实现；测试和已完成附件装载的调用方可直接复用。
pub async fn edit_message_and_truncate_history_with_loaded_attachments(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    anchor_message_id: String,
    message: ChatMessage,
) -> Result<EditMessageMutationResult, String> {
    let gate = attachment_gc_gate().read().await;
    let key = topic_key(owner_id, owner_type, &topic_id)?;
    edit_message_and_truncate_history_with_loaded_attachments_with_gate(
        db_pool,
        &key,
        anchor_message_id,
        message,
        None,
        &gate,
    )
    .await
}

async fn edit_message_and_truncate_history_with_loaded_attachments_with_gate(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &crate::vcp_modules::topic_types::TopicKey,
    anchor_message_id: String,
    message: ChatMessage,
    roots: Option<&crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots>,
    gate: &crate::vcp_modules::infra::file_manager::AttachmentReadGuard,
) -> Result<EditMessageMutationResult, String> {
    edit::edit_message_and_truncate_history_with_loaded_attachments_with_gate(
        db_pool,
        key,
        anchor_message_id,
        message,
        roots,
        gate,
    )
    .await
}

async fn register_active_generation(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &crate::vcp_modules::topic_types::TopicKey,
    message: &ChatMessage,
) -> Result<(), String> {
    sqlx::query(
        "INSERT OR REPLACE INTO active_generations
            (owner_type, owner_id, topic_id, msg_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(&message.id)
    .bind(message.timestamp as i64)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

async fn refresh_message_topic(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &crate::vcp_modules::topic_types::TopicKey,
    message: &ChatMessage,
) -> Result<(), String> {
    let msg_count: i32 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "UPDATE topics
         SET updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?),
             msg_count = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(message.timestamp as i64)
    .bind(message.updated_at.unwrap_or(message.timestamp) as i64)
    .bind(msg_count)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Reads an uncompressed message body using the complete Wire 1.4 identity.
#[tauri::command]
pub async fn fetch_raw_message_content(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
) -> Result<String, String> {
    fetch_raw_message_content_for_key(&app_handle, &owner_type, &owner_id, &topic_id, &message_id)
        .await
}

/// Wire 1.4 raw-content lookup with explicit owner/topic identity.
pub async fn fetch_raw_message_content_for_key(
    app_handle: &tauri::AppHandle,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    message_id: &str,
) -> Result<String, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let key = topic_key(owner_id, owner_type, topic_id)?;
    let row = sqlx::query(
        "SELECT content FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&db_state.pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| {
        format!(
            "Message {}/{}/{}/{} not found",
            key.owner_type, key.owner_id, key.topic_id, message_id
        )
    })?;
    decode_message_content(&row, "content")
}

/// Rebuilds a render-cache entry using the complete Wire 1.4 identity.
#[tauri::command]
pub async fn re_render_message(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    message_id: String,
    topic_id: String,
) -> Result<serde_json::Value, String> {
    re_render_message_for_key(&app_handle, &owner_type, &owner_id, &topic_id, &message_id).await
}

/// Wire 1.4 render-cache rebuild with full composite message identity.
pub async fn re_render_message_for_key(
    app_handle: &tauri::AppHandle,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    message_id: &str,
) -> Result<serde_json::Value, String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let key = topic_key(owner_id, owner_type, topic_id)?;
    let row = sqlx::query(
        "SELECT content, content_hash FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_optional(&db_state.pool)
    .await
    .map_err(|error| error.to_string())?
    .ok_or_else(|| format!("Message {} with topic {} not found", message_id, topic_id))?;
    let content = decode_message_content(&row, "content")?;
    let compiled = MessageRenderCompiler::compile(&content);
    let serialized = MessageRenderCompiler::serialize(&compiled)?;
    let content_hash: String = row
        .try_get("content_hash")
        .map_err(|error| error.to_string())?;
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO render_cache (
            owner_type, owner_id, topic_id, msg_id, render_content,
            content_hash, renderer_schema_version, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id, msg_id) DO UPDATE SET
            render_content = excluded.render_content,
            content_hash = excluded.content_hash,
            renderer_schema_version = excluded.renderer_schema_version,
            updated_at = excluded.updated_at",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(&serialized)
    .bind(&content_hash)
    .bind(crate::vcp_modules::message_repository::RENDERER_SCHEMA_VERSION)
    .bind(now)
    .execute(&db_state.pool)
    .await
    .map_err(|error| error.to_string())?;
    serde_json::to_value(compiled).map_err(|error| error.to_string())
}

pub async fn patch_single_message<R: tauri::Runtime>(
    app_handle: AppHandle<R>,
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    mut message: ChatMessage,
    skip_bubble: bool,
) -> Result<Vec<ContentBlock>, String> {
    let gate = attachment_gc_gate().read().await;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(&app_handle)?;
    ensure_attachments_locally(&app_handle, &mut message).await?;
    let key = topic_key(owner_id, owner_type, &topic_id)?;
    patch_single_message_with_loaded_attachments_with_gate(
        db_pool,
        &key,
        message,
        skip_bubble,
        Some(&roots),
        &gate,
    )
    .await
}

/// 更新已完成附件装载的正文，并统一刷新哈希、渲染缓存和全文索引。
pub async fn patch_single_message_with_loaded_attachments(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    owner_id: &str,
    owner_type: &str,
    topic_id: String,
    message: ChatMessage,
    skip_bubble: bool,
) -> Result<Vec<ContentBlock>, String> {
    let gate = attachment_gc_gate().read().await;
    let key = topic_key(owner_id, owner_type, &topic_id)?;
    patch_single_message_with_loaded_attachments_with_gate(
        db_pool,
        &key,
        message,
        skip_bubble,
        None,
        &gate,
    )
    .await
}

async fn patch_single_message_with_loaded_attachments_with_gate(
    db_pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &crate::vcp_modules::topic_types::TopicKey,
    message: ChatMessage,
    skip_bubble: bool,
    roots: Option<&crate::vcp_modules::infra::maintenance_manager::ManagedAttachmentRoots>,
    gate: &crate::vcp_modules::infra::file_manager::AttachmentReadGuard,
) -> Result<Vec<ContentBlock>, String> {
    let blocks = if let Some(blocks) = &message.blocks {
        serde_json::from_value(blocks.clone()).map_err(|error| error.to_string())?
    } else {
        MessageRenderCompiler::compile(&message.content)
    };
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    let mut tx = db_pool.begin().await.map_err(|error| error.to_string())?;
    MessageRepository::upsert_message_for_topic_with_attachment_gate_and_roots(
        &mut tx,
        &message,
        key,
        &render_bytes,
        skip_bubble,
        gate,
        roots,
    )
    .await?;
    sqlx::query(
        "UPDATE topics
         SET updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(chrono::Utc::now().timestamp_millis())
    .bind(message.updated_at.unwrap_or(message.timestamp) as i64)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(blocks)
}
