use super::super::message_service_support::topic_key;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::SqlitePool;
use tauri::{Manager, Runtime};

/// 使用完整消息身份删除一个附件关联。
#[tauri::command]
pub async fn delete_message_attachment(
    app_handle: tauri::AppHandle,
    owner_id: String,
    owner_type: String,
    topic_id: String,
    message_id: String,
    hash: String,
) -> Result<(), String> {
    delete_message_attachment_for_key(
        &app_handle,
        &owner_type,
        &owner_id,
        &topic_id,
        &message_id,
        &hash,
    )
    .await
}

/// 按 owner 写入附件删除标记并刷新话题哈希。
pub async fn delete_message_attachment_for_key<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    message_id: &str,
    hash: &str,
) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let key = topic_key(owner_id, owner_type, topic_id)?;
    delete_message_attachment_in_pool(&db_state.pool, &key, message_id, hash).await
}

pub(crate) async fn delete_message_attachment_in_pool(
    pool: &SqlitePool,
    key: &TopicKey,
    message_id: &str,
    hash: &str,
) -> Result<(), String> {
    let now = crate::vcp_modules::infra::utils::now_millis();
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    let changed = sqlx::query(
        "UPDATE message_attachments
         SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id = ? AND hash = ?",
    )
    .bind(now)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(hash)
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    if changed.rows_affected() != 1 {
        return Err(format!("附件 {hash} 不存在、消息身份不完整或关联不唯一"));
    }
    super::bubble_topic_hash(&mut tx, &key).await?;
    tx.commit().await.map_err(|error| error.to_string())
}
