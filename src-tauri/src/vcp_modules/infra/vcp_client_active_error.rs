use super::super::{CompletionLease, GuardedTransition};
use super::delete_active_generation_for_key;
use crate::vcp_modules::chat::topic_types::MessageKey;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use tauri::{AppHandle, Runtime};

pub(crate) async fn mark_message_as_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    custom_error: Option<String>,
) -> Result<(), String> {
    mark_message_as_error_inner(app_handle, pool, key, custom_error, None, None, None).await
}

/// 在完成租约仍属于当前请求时写入错误终态；旧请求只会得到 `Skipped`。
pub(crate) async fn mark_message_as_error_guarded<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    lease: &CompletionLease,
    custom_error: Option<String>,
) -> Result<GuardedTransition<()>, String> {
    let app_handle = app_handle.clone();
    let pool = pool.clone();
    lease
        .with_current_transition(|key| async move {
            mark_message_as_error_inner(&app_handle, &pool, &key, custom_error, None, None, None)
                .await
        })
        .await
}

/// 使用调用方确认过的真实 session generation 写入错误终态；未知时保持缺省，禁止伪造。
pub(crate) async fn mark_message_as_error_guarded_with_generation<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    lease: &CompletionLease,
    custom_error: Option<String>,
    generation: Option<u64>,
) -> Result<GuardedTransition<()>, String> {
    let app_handle = app_handle.clone();
    let pool = pool.clone();
    lease
        .with_current_transition(|key| async move {
            mark_message_as_error_inner(
                &app_handle,
                &pool,
                &key,
                custom_error,
                None,
                generation,
                None,
            )
            .await
        })
        .await
}

/// 在同一租约内写入错误终态，并向流通道传播终结事件。
pub(crate) async fn mark_message_as_error_guarded_with_channel<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    lease: &CompletionLease,
    custom_error: Option<String>,
    stream_channel: Option<tauri::ipc::Channel<super::super::StreamEvent>>,
) -> Result<GuardedTransition<()>, String> {
    let app_handle = app_handle.clone();
    let pool = pool.clone();
    lease
        .with_current_transition(|key| async move {
            mark_message_as_error_inner(
                &app_handle,
                &pool,
                &key,
                custom_error,
                stream_channel,
                None,
                Some(lease.epoch()),
            )
            .await
        })
        .await
}

async fn mark_message_as_error_inner<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    custom_error: Option<String>,
    stream_channel: Option<tauri::ipc::Channel<super::super::StreamEvent>>,
    helper_generation: Option<u64>,
    event_generation: Option<u64>,
) -> Result<(), String> {
    let existing_content = load_existing_message_content(pool, key).await?;
    let active_generation = load_active_helper_generation(pool, key).await?;
    let expected_helper_generation = helper_generation.or_else(|| active_generation.flatten());
    if let Some(expected_helper_generation) = expected_helper_generation {
        if active_generation != Some(Some(expected_helper_generation)) {
            return Err("错误终结 generation 与活动记录不一致".to_string());
        }
        compare_and_confirm_helper_generation(pool, key, expected_helper_generation).await?;
    }
    if active_generation.is_some() {
        finalize_active_error(
            app_handle,
            pool,
            key,
            existing_content,
            custom_error,
            stream_channel,
            event_generation,
        )
        .await?;
    } else {
        persist_inactive_error(app_handle, pool, key, existing_content, custom_error).await?;
    }
    Ok(())
}

async fn compare_and_confirm_helper_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    generation: u64,
) -> Result<(), String> {
    let generation = i64::try_from(generation)
        .map_err(|_| "helper generation 超出 SQLite 整数范围".to_string())?;
    let result = sqlx::query(
        "UPDATE active_generations SET helper_generation = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND helper_generation = ?",
    )
    .bind(generation)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(generation)
    .execute(pool)
    .await
    .map_err(|error| format!("错误终结 generation CAS 失败: {error}"))?;
    if result.rows_affected() != 1 {
        return Err("错误终结 generation 与活动记录不一致".to_string());
    }
    Ok(())
}

async fn load_active_helper_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<Option<Option<u64>>, String> {
    let row = sqlx::query(
        "SELECT helper_generation FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("读取活动 helper generation 失败: {error}"))?;
    row.map(|row| {
        use sqlx::Row;
        row.try_get::<Option<i64>, _>("helper_generation")
            .map_err(|error| format!("解析活动 helper generation 失败: {error}"))?
            .map(|value| {
                u64::try_from(value).map_err(|_| "活动 helper generation 不是正整数".to_string())
            })
            .transpose()
    })
    .transpose()
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

async fn finalize_active_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    existing_content: String,
    custom_error: Option<String>,
    stream_channel: Option<tauri::ipc::Channel<super::super::StreamEvent>>,
    generation: Option<u64>,
) -> Result<(), String> {
    use sqlx::Row;

    let agent_id = match sqlx::query(
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
    {
        Some(row) => row
            .try_get::<Option<String>, _>("agent_id")
            .map_err(|error| format!("读取消息代理失败: {error}"))?,
        None => None,
    };
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
        stream_channel,
        agent_id,
        generation,
    )
    .await
}

async fn persist_inactive_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    existing_content: String,
    custom_error: Option<String>,
) -> Result<(), String> {
    let final_content = append_error_suffix(existing_content, custom_error.as_deref());
    crate::vcp_modules::chat::message_service::update_existing_message_content(
        app_handle.clone(),
        pool,
        key,
        final_content,
        Some("error".to_string()),
        false,
    )
    .await?;
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
