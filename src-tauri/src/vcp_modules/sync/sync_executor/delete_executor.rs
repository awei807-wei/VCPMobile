use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::ExpectedMessageStates;
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use crate::vcp_modules::sync_types::{MessageDeleteDecision, SYNC_TOMBSTONE_HASH};
use crate::vcp_modules::topic_types::{MessageKey, OwnerKey, TopicKey};
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

#[path = "delete_executor_messages.rs"]
mod messages;
#[path = "delete_executor_storage.rs"]
mod storage;
use messages::soft_delete_messages_data;
use storage::{soft_delete_owner_data, soft_delete_topic_data};

#[cfg(test)]
#[path = "delete_executor_tests.rs"]
mod tests;

const MAX_SAFE_TIMESTAMP: i64 = (1_i64 << 53) - 1;

fn validate_deleted_at(deleted_at: i64, entity: &str) -> Result<(), String> {
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&deleted_at) {
        return Err(format!(
            "{entity} delete requires a safe non-negative deletedAt"
        ));
    }
    Ok(())
}

fn cancel_active_requests<R: Runtime>(app: &AppHandle<R>, keys: Vec<MessageKey>) {
    let Some(active_requests) = app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
    else {
        return;
    };
    for key in keys {
        if let Some((_, cancel_sender)) = active_requests.0.remove_key(&key) {
            let _ = cancel_sender.send(());
        }
    }
}

impl DeleteExecutor {
    /// Delete an owner namespace. The fixed Agent/Group wrappers are retained
    /// for business commands, while storage receives a complete OwnerKey.
    pub async fn soft_delete_agent<R: Runtime>(
        app: &AppHandle<R>,
        agent_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        Self::soft_delete_owner(app, &OwnerKey::new("agent", agent_id), deleted_at).await
    }

    pub async fn soft_delete_group<R: Runtime>(
        app: &AppHandle<R>,
        group_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        Self::soft_delete_owner(app, &OwnerKey::new("group", group_id), deleted_at).await
    }

    pub async fn soft_delete_owner<R: Runtime>(
        app: &AppHandle<R>,
        key: &OwnerKey,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Owner")?;
        let db = app.state::<DbState>();
        let receipt = soft_delete_owner_data(&db.pool, key, deleted_at).await?;
        invalidate_owner_cache(app, key);
        cancel_active_requests(app, receipt.active_messages);
        Ok(())
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        key: &TopicKey,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Topic")?;
        let db = app.state::<DbState>();
        let receipt = soft_delete_topic_data(&db.pool, key, deleted_at).await?;
        cancel_active_requests(app, receipt.active_messages);
        Ok(())
    }

    /// Apply one remote message tombstone using a complete MessageKey.
    pub async fn soft_delete_message<R: Runtime>(
        app: &AppHandle<R>,
        key: &MessageKey,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Message")?;
        if !key.is_valid() {
            return Err("Message delete requires a complete message identity".to_string());
        }
        let decision = MessageDeleteDecision {
            msg_id: key.msg_id.clone(),
            deleted_at,
        };
        let db = app.state::<DbState>();
        let receipt =
            soft_delete_messages_data(&db.pool, &key.topic, &[decision], true, None).await?;
        cancel_active_requests(app, receipt.active_messages);
        Ok(())
    }

    /// Apply a batch of Desktop message tombstones. Aggregate topic/owner
    /// repair is deliberately deferred to SyncFinalizer for one-pass repair.
    pub async fn soft_delete_messages<R: Runtime>(
        app: &AppHandle<R>,
        key: &TopicKey,
        tombstones: &[MessageDeleteDecision],
        expected_states: &ExpectedMessageStates,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let receipt =
            soft_delete_messages_data(&db.pool, key, tombstones, false, Some(expected_states))
                .await?;
        cancel_active_requests(app, receipt.active_messages);
        Ok(())
    }

    pub async fn soft_delete_avatar<R: Runtime>(
        app: &AppHandle<R>,
        owner_type: &str,
        owner_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Avatar")?;
        if !crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) {
            return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
        }
        let db = app.state::<DbState>();
        let current: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT deleted_at FROM avatars WHERE owner_type = ? AND owner_id = ?",
        )
        .bind(owner_type)
        .bind(owner_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|error| error.to_string())?;
        match current {
            None => insert_avatar_tombstone(&db.pool, owner_type, owner_id, deleted_at).await,
            Some(Some(existing)) if existing >= deleted_at => Ok(()),
            Some(Some(_)) | Some(None) => {
                let result = sqlx::query(
                    "UPDATE avatars SET deleted_at = MAX(COALESCE(deleted_at, 0), ?)
                     WHERE owner_type = ? AND owner_id = ?",
                )
                .bind(deleted_at)
                .bind(owner_type)
                .bind(owner_id)
                .execute(&db.pool)
                .await
                .map_err(|error| error.to_string())?;
                if result.rows_affected() != 1 {
                    return Err(format!(
                        "Avatar {owner_type}/{owner_id} disappeared during delete"
                    ));
                }
                Ok(())
            }
        }
    }

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        if days < 0 {
            return Err("cleanup days must be non-negative".to_string());
        }
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;
        let render_cache = sqlx::query(
            "DELETE FROM render_cache
             WHERE (owner_type, owner_id, topic_id, msg_id) IN (
                SELECT owner_type, owner_id, topic_id, msg_id FROM messages
                WHERE deleted_at IS NOT NULL AND deleted_at < ?
             )",
        )
        .bind(threshold)
        .execute(&db.pool)
        .await
        .map_err(|error| error.to_string())?;
        let cleared_content = ContentCompressor::compress("[已清空]")?;
        let messages = sqlx::query(
            "UPDATE messages SET content = ?
             WHERE deleted_at IS NOT NULL AND deleted_at < ? AND content != ?",
        )
        .bind(&cleared_content)
        .bind(threshold)
        .bind(&cleared_content)
        .execute(&db.pool)
        .await
        .map_err(|error| error.to_string())?;
        log::info!(
            "[DeleteExecutor] Cleanup older than {days} days: cleared_messages={}, deleted_render_caches={}",
            messages.rows_affected(),
            render_cache.rows_affected()
        );
        Ok(())
    }
}

fn invalidate_owner_cache<R: Runtime>(app: &AppHandle<R>, key: &OwnerKey) {
    match key.owner_type.as_str() {
        "agent" => {
            if let Some(state) =
                app.try_state::<crate::vcp_modules::agent_service::AgentConfigState>()
            {
                state.caches.remove(&key.owner_id);
                state.locks.remove(&key.owner_id);
            }
        }
        "group" => {
            if let Some(state) =
                app.try_state::<crate::vcp_modules::group_service::GroupManagerState>()
            {
                state.caches.remove(&key.owner_id);
                state.locks.remove(&key.owner_id);
            }
        }
        _ => {}
    }
}

async fn insert_avatar_tombstone(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
    deleted_at: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO avatars (
            owner_type, owner_id, avatar_hash, mime_type, image_data,
            updated_at, deleted_at
         ) VALUES (?, ?, ?, 'application/octet-stream', ?, ?, ?)",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(SYNC_TOMBSTONE_HASH)
    .bind(Vec::<u8>::new())
    .bind(deleted_at)
    .bind(deleted_at)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|error| format!("写入头像墓碑失败: {error}"))
}
