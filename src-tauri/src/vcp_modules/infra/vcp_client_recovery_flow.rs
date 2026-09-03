#[cfg(target_os = "android")]
use super::super::transport::{get_helper_port, send_stop_to_helper};
use super::super::{active::mark_message_as_error, db_pool_if_ready, registry, ActiveRequests};
use super::{legacy_recovery_file_is_unambiguous, resolve_legacy_generation_key, support};
use crate::vcp_modules::chat::topic_types::MessageKey;
#[cfg(target_os = "android")]
use futures_util::StreamExt;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
#[cfg(target_os = "android")]
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

#[tauri::command]
pub async fn recover_active_generation<R: Runtime>(
    app: AppHandle<R>,
    active_requests: tauri::State<'_, ActiveRequests>,
    msg_id: String,
    owner_id: Option<String>,
    owner_type: Option<String>,
    topic_id: Option<String>,
) -> Result<Value, String> {
    let explicit_key = registry::optional_message_key(&msg_id, owner_id, owner_type, topic_id)?;
    log::info!(
        "[VCPClient] recover_active_generation called for msg_id: {}, scoped: {}",
        msg_id,
        explicit_key.is_some()
    );
    let pool = db_pool_if_ready(&app)?;
    let request_key = match explicit_key {
        Some(key) => key,
        None => resolve_legacy_generation_key(&pool, &msg_id).await?,
    };
    if active_requests.0.contains_key(&request_key) {
        log::info!(
            "[VCPClient] Active generation {}/{}/{}/{} is running in background. Returning streaming status.",
            request_key.topic.owner_type,
            request_key.topic.owner_id,
            request_key.topic.topic_id,
            request_key.msg_id
        );
        return Ok(json!({ "status": "streaming" }));
    }
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| error.to_string())?;
    let cleanup_dir = cache_dir.clone();
    tokio::spawn(async move { support::clean_old_cache_files(&cleanup_dir) });
    if let Some(result) = recover_from_disk(&app, &pool, &cache_dir, &request_key, &msg_id).await? {
        return Ok(result);
    }
    #[cfg(target_os = "android")]
    if let Some(result) = recover_from_helper(&app, &pool, &request_key, &msg_id).await {
        return Ok(result);
    }
    log::warn!(
        "[VCPClient] Active generation {} not found in active_requests and no local cache available. Marking as failed.",
        msg_id
    );
    mark_message_as_error(
        &app,
        &pool,
        &request_key,
        Some("后台进程已被系统销毁，流式对话中断".to_string()),
    )
    .await?;
    Ok(json!({ "status": "failed" }))
}

struct RecoveryPayload {
    content: String,
    finish_reason: Option<String>,
}

async fn recover_from_disk<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &std::path::Path,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Option<Value>, String> {
    let Some(path) = find_recovery_file(pool, cache_dir, key, msg_id).await? else {
        return Ok(None);
    };
    log::info!(
        "[VCPClient] Found local recovery file for composite identity {}/{}/{}/{}.",
        key.topic.owner_type,
        key.topic.owner_id,
        key.topic.topic_id,
        key.msg_id
    );
    let Some(payload) = read_recovery_payload(&path) else {
        return Ok(None);
    };
    let timestamp = payload.1;
    if chrono::Utc::now().timestamp_millis() - timestamp > 24 * 3600 * 1000 {
        log::warn!("[VCPClient] Recovered JSON file is older than 24 hours. Deleting and failing.");
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    let payload = RecoveryPayload {
        content: payload.0,
        finish_reason: payload.2,
    };
    finalize_if_active(app, pool, key, &payload).await?;
    let _ = std::fs::remove_file(path);
    Ok(Some(
        json!({"status": "completed", "content": payload.content}),
    ))
}

async fn find_recovery_file(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &std::path::Path,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let identity =
        serde_json::to_vec(key).map_err(|error| format!("serialize recovery identity: {error}"))?;
    let composite = cache_dir.join("sse_cache").join(format!(
        "sse_recovered_{}.json",
        crate::vcp_modules::infra::utils::calculate_sha256(&identity)
    ));
    if composite.exists() {
        return Ok(Some(composite));
    }
    let legacy = cache_dir.join("sse_cache").join(format!(
        "sse_recovered_{}.json",
        crate::vcp_modules::infra::utils::calculate_sha256(msg_id.as_bytes())
    ));
    if legacy.exists() && legacy_recovery_file_is_unambiguous(pool, key).await? {
        return Ok(Some(legacy));
    }
    Ok(None)
}

fn read_recovery_payload(path: &std::path::Path) -> Option<(String, i64, Option<String>)> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    Some((
        value["content"].as_str().unwrap_or("").to_string(),
        value["timestamp"].as_i64().unwrap_or(0),
        value["finishReason"].as_str().map(str::to_string),
    ))
}

async fn finalize_if_active<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    payload: &RecoveryPayload,
) -> Result<(), String> {
    let active = sqlx::query(
        "SELECT 1 FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| error.to_string())?;
    if active.is_none() {
        return Ok(());
    }
    let agent_id = load_agent_id(pool, key).await?;
    crate::vcp_modules::chat::message_service::finalize_stream_message(
        app.clone(),
        pool,
        &key.topic.owner_id,
        &key.topic.owner_type,
        key.topic.topic_id.clone(),
        key.msg_id.clone(),
        payload.content.clone(),
        false,
        payload
            .finish_reason
            .clone()
            .or(Some("completed".to_string())),
        None,
        agent_id,
    )
    .await
}

async fn load_agent_id(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<Option<String>, String> {
    use sqlx::Row;

    sqlx::query(
        "SELECT agent_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map(|row| row.and_then(|row| row.get::<Option<String>, _>("agent_id")))
    .map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
async fn recover_from_helper<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
) -> Option<Value> {
    log::info!("[VCPClient] Querying helper process via TCP for msg_id: {msg_id}");
    match query_helper_session(app, key, msg_id).await {
        Ok(response) => handle_helper_response(app, pool, key, msg_id, response)
            .await
            .ok()
            .flatten(),
        Err(error) => {
            log::warn!("[VCPClient] Failed to query helper via TCP socket: {error}");
            None
        }
    }
}

#[cfg(target_os = "android")]
async fn query_helper_session<R: Runtime>(
    app: &AppHandle<R>,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Value, String> {
    use tokio::io::AsyncWriteExt;

    let port = get_helper_port(app)?;
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .map_err(|error| format!("TCP connection failed: {error}"))?;
    let command = json!({
        "action": "query",
        "requestId": msg_id,
        "ownerType": key.topic.owner_type,
        "ownerId": key.topic.owner_id,
        "topicId": key.topic.topic_id
    });
    let bytes = command.to_string();
    let length = (bytes.len() as u32).to_be_bytes();
    stream
        .write_all(&length)
        .await
        .map_err(|error| error.to_string())?;
    stream
        .write_all(bytes.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream.flush().await.map_err(|error| error.to_string())?;
    let mut reader = FramedRead::new(stream, LengthDelimitedCodec::new());
    let frame = reader
        .next()
        .await
        .ok_or_else(|| "No query response received (EOF)".to_string())?
        .map_err(|error| error.to_string())?;
    serde_json::from_slice::<Value>(&frame).map_err(|error| error.to_string())
}

#[cfg(target_os = "android")]
async fn handle_helper_response<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    msg_id: &str,
    response: Value,
) -> Result<Option<Value>, String> {
    let status = response["status"].as_str().unwrap_or("not_found");
    let content = response["content"].as_str().unwrap_or("").to_string();
    let finish_reason = response["lastFinishReason"].as_str().map(str::to_string);
    log::info!(
        "[VCPClient] Query response received: status={status}, content_len={}, finish_reason={finish_reason:?}",
        content.len()
    );
    match status {
        "completed" => {
            finalize_if_active(
                app,
                pool,
                key,
                &RecoveryPayload {
                    content: content.clone(),
                    finish_reason: finish_reason.or(Some("completed".to_string())),
                },
            )
            .await?;
            let _ = send_stop_to_helper(app, msg_id, key).await;
            Ok(Some(json!({"status": "completed", "content": content})))
        }
        "streaming" => Ok(Some(json!({
            "status": "streaming",
            "content": content,
            "lastEventIndex": response["lastEventIndex"]
        }))),
        _ => {
            log::warn!("[VCPClient] Session status is 'not_found' in helper.");
            Ok(None)
        }
    }
}
