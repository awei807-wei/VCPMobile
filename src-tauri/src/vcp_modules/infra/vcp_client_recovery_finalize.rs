use super::{CompletionLease, GuardedTransition, MessageKey, RecoveryFinalization};
use crate::vcp_modules::chat::chat_manager::ChatMessage;
use crate::vcp_modules::message_repository::{MessageRenderCompiler, MessageRepository};
use sqlx::{Row, Sqlite, Transaction};
use std::path::Path;
use tauri::{AppHandle, Runtime};

pub(super) struct RecoveryPayload {
    pub(super) content: String,
    pub(super) finish_reason: Option<String>,
    pub(super) generation: u64,
}

pub(super) async fn active_helper_generation(
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
        row.try_get::<Option<i64>, _>("helper_generation")
            .map_err(|error| format!("解析活动 helper generation 失败: {error}"))
            .and_then(|generation| {
                generation
                    .map(|value| {
                        u64::try_from(value)
                            .map_err(|_| "活动 helper generation 不是正整数".to_string())
                    })
                    .transpose()
            })
    })
    .transpose()
}

pub(super) async fn bind_active_helper_generation(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    generation: u64,
) -> Result<bool, String> {
    let generation = i64::try_from(generation)
        .map_err(|_| "helper generation 超出 SQLite 整数范围".to_string())?;
    let result = sqlx::query(
        "UPDATE active_generations SET helper_generation = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND (helper_generation IS NULL OR helper_generation = ?)",
    )
    .bind(generation)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(generation)
    .execute(pool)
    .await
    .map_err(|error| format!("绑定活动 helper generation 失败: {error}"))?;
    Ok(result.rows_affected() == 1)
}

pub(super) async fn delete_active_generation_if_observed(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    observed_generation: Option<u64>,
) -> Result<bool, String> {
    let mut query = String::from(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    );
    if observed_generation.is_some() {
        query.push_str(" AND helper_generation = ?");
    } else {
        query.push_str(" AND helper_generation IS NULL");
    }
    let mut statement = sqlx::query(&query)
        .bind(&key.topic.owner_type)
        .bind(&key.topic.owner_id)
        .bind(&key.topic.topic_id)
        .bind(&key.msg_id);
    if let Some(generation) = observed_generation {
        statement = statement.bind(
            i64::try_from(generation)
                .map_err(|_| "活动 helper generation 超出 SQLite 整数范围".to_string())?,
        );
    }
    let result = statement
        .execute(pool)
        .await
        .map_err(|error| format!("自愈清理活动 generation 失败: {error}"))?;
    Ok(result.rows_affected() == 1)
}

pub(super) async fn message_has_terminal_state(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
) -> Result<bool, String> {
    let row = sqlx::query(
        "SELECT finish_reason FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("读取消息终态失败: {error}"))?;
    let Some(row) = row else {
        return Ok(false);
    };
    let finish_reason = row
        .try_get::<Option<String>, _>("finish_reason")
        .map_err(|error| format!("读取消息结束原因失败: {error}"))?;
    Ok(finish_reason.is_some_and(|reason| !reason.is_empty()))
}

pub(super) async fn finalize_if_active<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    payload: &RecoveryPayload,
    recovery_lease: &CompletionLease,
) -> Result<RecoveryFinalization, String> {
    let app = app.clone();
    let pool = pool.clone();
    let payload = RecoveryPayload {
        content: payload.content.clone(),
        finish_reason: payload.finish_reason.clone(),
        generation: payload.generation,
    };
    let transition = recovery_lease
        .with_current_transition(|current_key| async move {
            finalize_if_active_under_transition(&app, &pool, &current_key, &payload).await
        })
        .await?;
    Ok(match transition {
        GuardedTransition::Applied(result) => result,
        GuardedTransition::Skipped => RecoveryFinalization::Skipped,
    })
}

pub(super) async fn finalize_if_active_under_transition<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    payload: &RecoveryPayload,
) -> Result<RecoveryFinalization, String> {
    let _ = app;
    finalize_if_active_atomically(pool, key, payload, None).await
}

/// 将恢复消息写入和精确删除 active 行放在同一个 SQLite 事务中。
/// cleanup_path 非空时，清理义务也在该事务中先写入 outbox。
pub(super) async fn finalize_if_active_with_cleanup(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    payload: &RecoveryPayload,
    cache_dir: &Path,
    cleanup_path: &Path,
) -> Result<RecoveryFinalization, String> {
    finalize_if_active_atomically(pool, key, payload, Some((cache_dir, cleanup_path))).await
}

async fn finalize_if_active_atomically(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    key: &MessageKey,
    payload: &RecoveryPayload,
    cleanup_path: Option<(&Path, &Path)>,
) -> Result<RecoveryFinalization, String> {
    if payload.generation == 0 {
        return Ok(RecoveryFinalization::Skipped);
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("恢复终结事务启动失败: {error}"))?;
    let active = active_helper_generation_tx(&mut tx, key).await?;
    if active.is_none() {
        return Ok(RecoveryFinalization::NotFound);
    }
    if active != Some(Some(payload.generation)) {
        log::warn!(
            "[VCPClient] 恢复 payload generation 与活动 generation 不一致，拒绝终结: expected={active:?}, actual={}",
            payload.generation
        );
        return Ok(RecoveryFinalization::Skipped);
    }
    if let Some((cache_dir, path)) = cleanup_path {
        super::files::enqueue_cleanup_debt(&mut tx, cache_dir, path, key, payload.generation)
            .await?;
    }
    let message = build_recovery_message(&mut tx, key, payload).await?;
    let blocks = MessageRenderCompiler::compile(&message.content);
    let render_bytes = MessageRenderCompiler::serialize(&blocks)?;
    MessageRepository::upsert_message_for_topic(
        &mut tx,
        &message,
        &key.topic,
        &render_bytes,
        false,
    )
    .await?;
    refresh_recovery_topic(&mut tx, key, message.timestamp, message.updated_at).await?;
    let deleted = delete_active_generation(&mut tx, key, payload.generation).await?;
    if !deleted {
        return Ok(RecoveryFinalization::Skipped);
    }
    tx.commit()
        .await
        .map_err(|error| format!("恢复终结事务提交失败: {error}"))?;
    Ok(RecoveryFinalization::Applied)
}

async fn active_helper_generation_tx(
    tx: &mut Transaction<'_, Sqlite>,
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
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("事务内读取活动 helper generation 失败: {error}"))?
    .map(|row| {
        row.try_get::<Option<i64>, _>("helper_generation")
            .map_err(|error| format!("事务内解析活动 helper generation 失败: {error}"))
            .and_then(|generation| {
                generation
                    .map(|value| {
                        u64::try_from(value)
                            .map_err(|_| "活动 helper generation 不是正整数".to_string())
                    })
                    .transpose()
            })
    })
    .transpose();
    row
}

async fn build_recovery_message(
    tx: &mut Transaction<'_, Sqlite>,
    key: &MessageKey,
    payload: &RecoveryPayload,
) -> Result<ChatMessage, String> {
    let row = sqlx::query(
        "SELECT agent_id FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("读取恢复消息代理失败: {error}"))?
    .ok_or_else(|| "恢复消息不存在或已删除".to_string())?;
    let stored_agent_id = row
        .try_get::<Option<String>, _>("agent_id")
        .map_err(|error| format!("读取恢复消息代理字段失败: {error}"))?;
    let is_group = key.topic.owner_type == "group";
    let agent_id = if is_group {
        stored_agent_id
    } else {
        Some(key.topic.owner_id.clone())
    };
    let name = load_agent_name_tx(tx, agent_id.as_deref()).await?;
    let timestamp = crate::vcp_modules::infra::utils::now_millis() as u64;
    Ok(ChatMessage {
        id: key.msg_id.clone(),
        role: "assistant".to_string(),
        name,
        content: payload.content.clone(),
        timestamp,
        updated_at: Some(timestamp),
        is_thinking: Some(false),
        agent_id,
        group_id: is_group.then(|| key.topic.owner_id.clone()),
        topic_id: Some(key.topic.topic_id.clone()),
        is_group_message: Some(is_group),
        finish_reason: payload
            .finish_reason
            .clone()
            .or(Some("completed".to_string())),
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: None,
    })
}

async fn load_agent_name_tx(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(agent_id) = agent_id else {
        return Ok(None);
    };
    sqlx::query("SELECT name FROM agents WHERE agent_id = ?")
        .bind(agent_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| format!("读取恢复代理名称失败: {error}"))?
        .map(|row| {
            row.try_get("name")
                .map_err(|error| format!("读取恢复代理名称字段失败: {error}"))
        })
        .transpose()
}

async fn refresh_recovery_topic(
    tx: &mut Transaction<'_, Sqlite>,
    key: &MessageKey,
    timestamp: u64,
    updated_at: Option<u64>,
) -> Result<(), String> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| format!("读取恢复消息数失败: {error}"))?;
    sqlx::query(
        "UPDATE topics
         SET updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?),
             msg_count = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(i64::try_from(timestamp).map_err(|_| "恢复消息时间戳超出范围".to_string())?)
    .bind(
        i64::try_from(updated_at.unwrap_or(timestamp))
            .map_err(|_| "恢复消息更新时间超出范围".to_string())?,
    )
    .bind(count)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|error| format!("更新恢复话题状态失败: {error}"))
}

async fn delete_active_generation(
    tx: &mut Transaction<'_, Sqlite>,
    key: &MessageKey,
    generation: u64,
) -> Result<bool, String> {
    let generation = i64::try_from(generation)
        .map_err(|_| "恢复 payload generation 超出 SQLite 整数范围".to_string())?;
    let result = sqlx::query(
        "DELETE FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND helper_generation = ?",
    )
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(generation)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("恢复终结 generation CAS 删除失败: {error}"))?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
#[path = "vcp_client_recovery_finalize_tests.rs"]
mod tests;
