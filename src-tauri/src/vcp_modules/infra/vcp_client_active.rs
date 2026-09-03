use super::{db_pool_if_ready, registry, ActiveRequests};
use crate::vcp_modules::chat::topic_types::MessageKey;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};

/// 中止请求 Command: interruptRequest
/// 通过 messageId 立即触发对应的 oneshot 信号
#[tauri::command]
#[allow(non_snake_case)]
pub fn interruptRequest(
    state: tauri::State<'_, ActiveRequests>,
    message_id: String,
    owner_id: Option<String>,
    owner_type: Option<String>,
    topic_id: Option<String>,
) -> Result<Value, String> {
    let explicit_key = registry::optional_message_key(&message_id, owner_id, owner_type, topic_id)?;
    log::info!(
        "[VCPClient] interruptRequest called for messageId: {}, scoped: {}. Active requests: {}",
        message_id,
        explicit_key.is_some(),
        state.0.len()
    );
    let removed = match explicit_key {
        Some(key) => state.0.remove_key(&key),
        None => state.0.remove(&message_id),
    };
    if let Some((removed_key, sender)) = removed {
        log::info!(
            "[VCPClient] Found AbortController for {}/{}/{}/{}, aborting...",
            removed_key.topic.owner_type,
            removed_key.topic.owner_id,
            removed_key.topic.topic_id,
            removed_key.msg_id
        );
        let _ = sender.send(());
        log::info!(
            "[VCPClient] Request interrupted for messageId: {}. Remaining active requests: {}",
            message_id,
            state.0.len()
        );
        Ok(json!({"success": true, "message": format!("Request {} interrupted", message_id)}))
    } else {
        log::warn!(
            "[VCPClient] No unambiguous active request found for messageId: {}",
            message_id
        );
        Err(format!(
            "Request {} not found or legacy messageId is ambiguous",
            message_id
        ))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActiveGeneration {
    pub msg_id: String,
    pub topic_id: String,
    pub owner_id: String,
    pub owner_type: String,
    pub created_at: i64,
}

#[tauri::command]
pub async fn get_active_generations(
    app: tauri::AppHandle,
    active_requests: tauri::State<'_, ActiveRequests>,
) -> Result<Vec<ActiveGeneration>, String> {
    let pool = db_pool_if_ready(&app)?;
    let rows = sqlx::query(
        "SELECT msg_id, topic_id, owner_id, owner_type, created_at FROM active_generations ORDER BY created_at ASC"
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let owner_id: String = row.get("owner_id");
        let owner_type: String = row.get("owner_type");
        let key = registry::message_key_from_parts(&owner_id, &owner_type, &topic_id, &msg_id)?;
        // 过滤掉当前正在活跃运行的后台流式任务，它们由 sse helper 代理，并不是“被异常打断”的
        if active_requests.0.contains_key(&key) {
            continue;
        }
        list.push(ActiveGeneration {
            msg_id,
            topic_id,
            owner_id,
            owner_type,
            created_at: row.get("created_at"),
        });
    }
    Ok(list)
}

/// Delete exactly one active generation. Every production caller must provide
/// the complete message identity so a shared `msg_id` cannot remove another
/// owner's recovery row.
pub(super) async fn delete_active_generation_for_key(
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
    .map_err(|error| error.to_string())
}

pub(crate) async fn mark_message_as_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    custom_error: Option<String>,
) -> Result<(), String> {
    let existing_content = load_existing_message_content(pool, key).await?;
    if has_active_generation(pool, key).await? {
        finalize_active_error(app_handle, pool, key, existing_content, custom_error).await?;
    } else {
        persist_inactive_error(pool, key, existing_content, custom_error).await?;
    }
    Ok(())
}

async fn load_existing_message_content(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<String, String> {
    let row = sqlx::query(
        "SELECT content FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    row.map(|row| decode_message_content(&row, "content"))
        .transpose()
        .map(|content| content.unwrap_or_default())
}

async fn has_active_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<bool, String> {
    sqlx::query(
        "SELECT 1 FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map(|row| row.is_some())
    .map_err(|error| error.to_string())
}

async fn finalize_active_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    existing_content: String,
    custom_error: Option<String>,
) -> Result<(), String> {
    use sqlx::Row;

    let agent_id = sqlx::query(
        "SELECT agent_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?
    .and_then(|row| row.get::<Option<String>, _>("agent_id"));
    let final_content = append_error_suffix(existing_content, custom_error.as_deref());
    crate::vcp_modules::chat::message_service::finalize_stream_message(
        app_handle.clone(),
        pool,
        &key.topic.owner_id,
        &key.topic.owner_type,
        key.topic.topic_id.clone(),
        key.msg_id.clone(),
        final_content,
        false,
        Some("error".to_string()),
        None,
        agent_id,
    )
    .await
}

async fn persist_inactive_error(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    existing_content: String,
    custom_error: Option<String>,
) -> Result<(), String> {
    let final_content = append_error_suffix(existing_content, custom_error.as_deref());
    sqlx::query(
        "UPDATE messages
         SET content = ?, finish_reason = 'error', is_thinking = 0
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(ContentCompressor::compress(&final_content)?)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(pool)
    .await
    .map_err(|error| error.to_string())?;
    delete_active_generation_for_key(pool, key).await
}

fn append_error_suffix(existing_content: String, custom_error: Option<&str>) -> String {
    let suffix = custom_error
        .map(|error| format!("\n\n> VCP流式错误: {error}"))
        .unwrap_or_else(|| "\n\n> VCP流式错误: 生成意外中断".to_string());
    if existing_content.is_empty() {
        suffix
    } else {
        format!("{existing_content}{suffix}")
    }
}
