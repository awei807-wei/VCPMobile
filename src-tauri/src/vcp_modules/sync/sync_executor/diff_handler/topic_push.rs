use super::action_dispatch::{failed_ids, send_failure};
use super::context::DiffContext;
use super::diff_item_validation::OwnerIdentity;
use super::phase;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_executor::PushExecutor;
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::{json, Value};
use sqlx::Row;
use tauri::Manager;

#[derive(Debug)]
pub(crate) struct TopicPushRequest {
    pub(crate) id: String,
    pub(crate) owner: OwnerIdentity,
}

pub(crate) async fn spawn_topic_push(ctx: &DiffContext, requests: Vec<TopicPushRequest>) {
    let context = ctx.clone();
    ctx.task_tracker
        .spawn(async move {
            let db = context.app_handle.state::<DbState>();
            let batch = match build_topic_push_batch(&db.pool, &requests).await {
                Ok(batch) => batch,
                Err(error) => {
                    send_failure(&context, error.code, error.message, error.failed_topic_ids);
                    return;
                }
            };
            for chunk in batch.chunks(1000) {
                let sub_batch = chunk.to_vec();
                let count = sub_batch.len() as u32;
                let failed_topic_ids = failed_ids(&sub_batch, &SyncDataType::Topic);
                if let Err(error) = PushExecutor::push_entities_batch(
                    &context.app_handle,
                    &context.http_client,
                    &context.base_url,
                    &context.token,
                    sub_batch,
                )
                .await
                {
                    send_failure(
                        &context,
                        "TOPIC_PUSH_FAILED",
                        format!("Batch topic push failed: {error}"),
                        failed_topic_ids,
                    );
                    return;
                }
                phase::complete_operations(&context, count);
            }
        })
        .await;
}

struct TopicPushFailure {
    code: String,
    message: String,
    failed_topic_ids: Vec<String>,
}

impl TopicPushFailure {
    fn one(code: &str, message: impl Into<String>, topic_id: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            failed_topic_ids: vec![topic_id.to_string()],
        }
    }
}

async fn build_topic_push_batch(
    pool: &sqlx::SqlitePool,
    requests: &[TopicPushRequest],
) -> Result<Vec<Value>, TopicPushFailure> {
    let mut batch = Vec::with_capacity(requests.len());
    for request in requests {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type
             FROM topics WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&request.id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            TopicPushFailure::one(
                "TOPIC_PUSH_DB_FAILED",
                format!("Failed to load topic {} for push: {error}", request.id),
                &request.id,
            )
        })?
        .ok_or_else(|| {
            TopicPushFailure::one(
                "TOPIC_PUSH_SOURCE_MISSING",
                format!("Topic selected for push is missing: {}", request.id),
                &request.id,
            )
        })?;
        let item = topic_push_item(&row, request).map_err(|message| {
            TopicPushFailure::one("TOPIC_PUSH_DB_DECODE_FAILED", message, &request.id)
        })?;
        batch.push(item);
    }
    Ok(batch)
}

fn topic_push_item(
    row: &sqlx::sqlite::SqliteRow,
    request: &TopicPushRequest,
) -> Result<Value, String> {
    let tid: String = row
        .try_get("topic_id")
        .map_err(|error| format!("topic id: {error}"))?;
    let title: String = row
        .try_get("title")
        .map_err(|error| format!("title: {error}"))?;
    let created_at: i64 = row
        .try_get("created_at")
        .map_err(|error| format!("created_at: {error}"))?;
    let locked: i64 = row
        .try_get("locked")
        .map_err(|error| format!("locked: {error}"))?;
    let unread: i64 = row
        .try_get("unread")
        .map_err(|error| format!("unread: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("owner_id: {error}"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("owner_type: {error}"))?;
    validate_topic_owner(request, &owner_type, &owner_id)?;
    let type_str = if owner_type == "group" {
        "group_topic"
    } else {
        "agent_topic"
    };
    let data = if owner_type == "group" {
        json!({ "id": tid, "name": title, "createdAt": created_at, "ownerId": owner_id })
    } else {
        json!({ "id": tid, "name": title, "createdAt": created_at, "locked": locked != 0, "unread": unread != 0, "ownerId": owner_id })
    };
    Ok(json!({ "id": request.id, "type": type_str, "data": data }))
}

pub(crate) fn validate_topic_owner(
    request: &TopicPushRequest,
    actual_type: &str,
    actual_id: &str,
) -> Result<(), String> {
    if actual_id != request.owner.owner_id || actual_type != request.owner.owner_type {
        return Err(format!(
            "Topic {} owner does not match the Phase 1 decision",
            request.id
        ));
    }
    Ok(())
}
