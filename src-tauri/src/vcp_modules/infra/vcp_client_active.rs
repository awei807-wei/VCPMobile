use super::{db_pool_if_ready, registry, ActiveRequests};
use crate::vcp_modules::chat::topic_types::MessageKey;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[path = "vcp_client_active_error.rs"]
mod error;
#[allow(unused_imports)]
pub(crate) use error::{
    mark_message_as_error, mark_message_as_error_guarded,
    mark_message_as_error_guarded_with_channel, mark_message_as_error_guarded_with_generation,
};

/// 中止请求 Command: interruptRequest
/// 通过 messageId 立即触发对应的 oneshot 信号
#[tauri::command]
#[allow(non_snake_case)]
pub async fn interruptRequest(
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
        Some(key) => state.0.remove_key_guarded(&key).await,
        None => state.0.remove_guarded(&message_id).await,
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
    pub helper_generation: Option<u64>,
}

#[tauri::command]
pub async fn get_active_generations(
    app: tauri::AppHandle,
    _active_requests: tauri::State<'_, ActiveRequests>,
) -> Result<Vec<ActiveGeneration>, String> {
    let pool = db_pool_if_ready(&app)?;
    load_active_generations(&pool).await
}

/// Load every persisted active generation.  The frontend uses this list after
/// a WebView reload, including rows whose Rust request registry is still alive:
/// those rows are precisely the ones that need helper query/resume takeover.
async fn load_active_generations(
    pool: &sqlx::Pool<sqlx::Sqlite>,
) -> Result<Vec<ActiveGeneration>, String> {
    let rows = sqlx::query(
        "SELECT msg_id, topic_id, owner_id, owner_type, created_at, helper_generation
         FROM active_generations ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;

    let mut list = Vec::new();
    for row in rows {
        use sqlx::Row;
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let owner_id: String = row.get("owner_id");
        let owner_type: String = row.get("owner_type");
        list.push(ActiveGeneration {
            msg_id,
            topic_id,
            owner_id,
            owner_type,
            created_at: row.get("created_at"),
            helper_generation: parse_helper_generation(&row)?,
        });
    }
    Ok(list)
}

fn parse_helper_generation(row: &sqlx::sqlite::SqliteRow) -> Result<Option<u64>, String> {
    use sqlx::Row;
    row.try_get::<Option<i64>, _>("helper_generation")
        .map_err(|error| format!("读取 helper generation 失败: {error}"))?
        .map(|value| u64::try_from(value).map_err(|_| "helper generation 不是正整数".to_string()))
        .transpose()
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

#[cfg(test)]
mod tests {
    use super::load_active_generations;

    #[tokio::test]
    async fn reload_discovery_includes_generation_even_when_request_is_still_active() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("创建内存数据库");
        sqlx::query(
            "CREATE TABLE active_generations (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                helper_generation INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
            )",
        )
        .execute(&pool)
        .await
        .expect("创建活动生成表");
        sqlx::query(
            "INSERT INTO active_generations
             (owner_type, owner_id, topic_id, msg_id, created_at, helper_generation)
             VALUES ('agent', 'owner-reload', 'topic-reload', 'message-reload', 7, 41)",
        )
        .execute(&pool)
        .await
        .expect("写入活动 generation");

        let rows = load_active_generations(&pool)
            .await
            .expect("WebView reload 应发现活动 generation");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "agent");
        assert_eq!(rows[0].owner_id, "owner-reload");
        assert_eq!(rows[0].topic_id, "topic-reload");
        assert_eq!(rows[0].msg_id, "message-reload");
        assert_eq!(rows[0].helper_generation, Some(41));
    }
}
