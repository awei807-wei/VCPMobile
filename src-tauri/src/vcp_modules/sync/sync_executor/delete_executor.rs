use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::persistence::message_repository::ContentCompressor;
use tauri::{AppHandle, Manager, Runtime};

pub struct DeleteExecutor;

#[path = "delete_storage.rs"]
mod storage;
use storage::{
    soft_delete_message_data, soft_delete_owner_data, soft_delete_topic_data, OwnerDeleteSpec,
};

fn validate_deleted_at(deleted_at: i64, entity: &str) -> Result<(), String> {
    if deleted_at < 0 {
        return Err(format!("{entity} delete requires a non-negative deletedAt"));
    }
    Ok(())
}

fn cancel_active_requests<R: Runtime>(app: &AppHandle<R>, message_ids: Vec<String>) {
    let Some(active_requests) = app.try_state::<crate::vcp_modules::vcp_client::ActiveRequests>()
    else {
        return;
    };
    for message_id in message_ids {
        if let Some((_, cancel_sender)) = active_requests.0.remove(&message_id) {
            let _ = cancel_sender.send(());
        }
    }
}

impl DeleteExecutor {
    pub async fn soft_delete_agent<R: Runtime>(
        app: &AppHandle<R>,
        agent_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Agent")?;
        let db = app.state::<DbState>();
        let active_ids = soft_delete_owner_data(
            &db.pool,
            OwnerDeleteSpec {
                table: "agents",
                id_column: "agent_id",
                owner_type: "agent",
            },
            agent_id,
            deleted_at,
        )
        .await?;
        if let Some(state) = app.try_state::<crate::vcp_modules::agent_service::AgentConfigState>()
        {
            state.caches.remove(agent_id);
            state.locks.remove(agent_id);
        }
        cancel_active_requests(app, active_ids);
        Ok(())
    }

    pub async fn soft_delete_group<R: Runtime>(
        app: &AppHandle<R>,
        group_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Group")?;
        let db = app.state::<DbState>();
        let active_ids = soft_delete_owner_data(
            &db.pool,
            OwnerDeleteSpec {
                table: "groups",
                id_column: "group_id",
                owner_type: "group",
            },
            group_id,
            deleted_at,
        )
        .await?;
        if let Some(state) = app.try_state::<crate::vcp_modules::group_service::GroupManagerState>()
        {
            state.caches.remove(group_id);
            state.locks.remove(group_id);
        }
        cancel_active_requests(app, active_ids);
        Ok(())
    }

    pub async fn soft_delete_topic<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Topic")?;
        let db = app.state::<DbState>();
        let active_ids = soft_delete_topic_data(&db.pool, topic_id, deleted_at).await?;
        cancel_active_requests(app, active_ids);
        Ok(())
    }

    pub async fn soft_delete_message<R: Runtime>(
        app: &AppHandle<R>,
        topic_id: &str,
        message_id: &str,
        deleted_at: i64,
    ) -> Result<(), String> {
        validate_deleted_at(deleted_at, "Message")?;
        if topic_id.is_empty() || message_id.is_empty() {
            return Err("Message delete requires topicId and id".to_string());
        }
        let db = app.state::<DbState>();
        if soft_delete_message_data(&db.pool, topic_id, message_id, deleted_at).await? {
            cancel_active_requests(app, vec![message_id.to_string()]);
        }
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
        if !matches!(current, Some(None)) {
            return Ok(());
        }
        let result = sqlx::query(
            "UPDATE avatars SET deleted_at = ?\
             WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL",
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

    pub async fn cleanup_old_deleted_records<R: Runtime>(
        app: &AppHandle<R>,
        days: i64,
    ) -> Result<(), String> {
        let db = app.state::<DbState>();
        let threshold = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;
        let render_cache = sqlx::query(
            "DELETE FROM render_cache WHERE (topic_id, msg_id) IN (\
               SELECT topic_id, msg_id FROM messages\
               WHERE deleted_at IS NOT NULL AND deleted_at < ?\
             )",
        )
        .bind(threshold)
        .execute(&db.pool)
        .await
        .map_err(|error| error.to_string())?;

        let cleared_content = ContentCompressor::compress("[已清空]")?;
        let messages = sqlx::query(
            "UPDATE messages SET content = ?\
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
