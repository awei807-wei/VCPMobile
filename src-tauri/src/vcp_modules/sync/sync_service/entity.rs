use super::attempt::{AttemptAction, AttemptContext};
use super::protocol::ENTITY_OPERATION_TIMEOUT;
use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
use crate::vcp_modules::sync_executor::PullExecutor;
use crate::vcp_modules::sync_types::{is_valid_avatar_owner, SyncDataType};
use serde_json::Value;
use std::sync::Arc;

pub(crate) async fn handle_entity_update(
    ctx: &mut AttemptContext,
    payload: &Value,
) -> AttemptAction {
    let id = match required_string(payload, "id") {
        Ok(value) => value,
        Err(message) => return protocol_failure(ctx, message, Vec::new()).await,
    };
    let data_type = match update_data_type(payload) {
        Ok(value) => value,
        Err(message) => return protocol_failure(ctx, message, vec![id]).await,
    };
    let owner_type = match topic_owner_type(payload, &data_type) {
        Ok(value) => value,
        Err(message) => return protocol_failure(ctx, message, vec![id]).await,
    };
    let app = ctx.app.clone();
    let http = ctx.http.clone();
    let base = ctx.http_url.clone();
    let token = ctx.token.clone();
    let queue = ctx.write_queue.clone();
    let semaphore = ctx.semaphore.clone();
    let operation_id = id.clone();
    let operation = async move {
        let _permit = semaphore.acquire().await?;
        pull_entity(EntityPullRequest {
            app: &app,
            http: &http,
            base: &base,
            token: &token,
            id: &operation_id,
            data_type,
            owner_type: &owner_type,
            queue: &queue,
        })
        .await?;
        queue
            .flush()
            .await
            .map_err(|error| format!("entity update write drain failed: {error}"))
    };
    match wait_for_operation(ctx, operation).await {
        Ok(()) => AttemptAction::Continue,
        Err(error) => {
            protocol_failure(
                ctx,
                format!("SYNC_ENTITY_UPDATE failed for {id}: {error}"),
                vec![id],
            )
            .await
        }
    }
}

struct EntityPullRequest<'a> {
    app: &'a tauri::AppHandle,
    http: &'a reqwest::Client,
    base: &'a str,
    token: &'a str,
    id: &'a str,
    data_type: SyncDataType,
    owner_type: &'a str,
    queue: &'a Arc<crate::vcp_modules::db_write_queue::DbWriteQueue>,
}

async fn pull_entity(request: EntityPullRequest<'_>) -> Result<(), String> {
    match request.data_type {
        SyncDataType::Agent => {
            PullExecutor::pull_agent(
                request.app,
                request.http,
                request.base,
                request.token,
                request.id,
                request.queue,
            )
            .await
        }
        SyncDataType::Group => {
            PullExecutor::pull_group(
                request.app,
                request.http,
                request.base,
                request.token,
                request.id,
                request.queue,
            )
            .await
        }
        SyncDataType::Topic if request.owner_type == "group" => {
            PullExecutor::pull_group_topic(
                request.app,
                request.http,
                request.base,
                request.token,
                request.id,
                request.queue,
            )
            .await
        }
        SyncDataType::Topic => {
            PullExecutor::pull_agent_topic(
                request.app,
                request.http,
                request.base,
                request.token,
                request.id,
                request.queue,
            )
            .await
        }
        _ => Err("unsupported entity update type".to_string()),
    }
}

pub(crate) async fn handle_entity_delete(
    ctx: &mut AttemptContext,
    payload: &Value,
) -> AttemptAction {
    let id = match required_string(payload, "id") {
        Ok(value) => value,
        Err(message) => return protocol_failure(ctx, message, Vec::new()).await,
    };
    let data_type = match delete_data_type(payload) {
        Ok(value) => value,
        Err(message) => return protocol_failure(ctx, message, vec![id]).await,
    };
    let deleted_at = match payload
        .get("deletedAt")
        .and_then(Value::as_i64)
        .filter(|v| *v >= 0)
    {
        Some(value) => value,
        None => {
            return protocol_failure(
                ctx,
                "SYNC_DELETE_NOTIFY requires a non-negative integer deletedAt".to_string(),
                vec![id],
            )
            .await
        }
    };
    let topic_id = if data_type == SyncDataType::Message {
        match required_string(payload, "topicId") {
            Ok(value) => Some(value),
            Err(message) => return protocol_failure(ctx, message, vec![id]).await,
        }
    } else {
        None
    };
    let app = ctx.app.clone();
    let operation_id = id.clone();
    let operation = async move {
        delete_entity(
            &app,
            &operation_id,
            data_type,
            deleted_at,
            topic_id.as_deref(),
        )
        .await
    };
    match wait_for_operation(ctx, operation).await {
        Ok(()) => AttemptAction::Continue,
        Err(error) => {
            protocol_failure(
                ctx,
                format!("SYNC_DELETE_NOTIFY failed for {id}: {error}"),
                vec![id],
            )
            .await
        }
    }
}

async fn delete_entity(
    app: &tauri::AppHandle,
    id: &str,
    data_type: SyncDataType,
    deleted_at: i64,
    topic_id: Option<&str>,
) -> Result<(), String> {
    match data_type {
        SyncDataType::Agent => DeleteExecutor::soft_delete_agent(app, id, deleted_at).await,
        SyncDataType::Group => DeleteExecutor::soft_delete_group(app, id, deleted_at).await,
        SyncDataType::Topic => DeleteExecutor::soft_delete_topic(app, id, deleted_at).await,
        SyncDataType::Avatar => {
            let Some((owner_type, owner_id)) = id.split_once(':') else {
                return Err(format!("invalid avatar id: {id}"));
            };
            if !is_valid_avatar_owner(owner_type, owner_id) {
                return Err(format!("invalid avatar id: {id}"));
            }
            DeleteExecutor::soft_delete_avatar(app, owner_type, owner_id, deleted_at).await
        }
        SyncDataType::Message => {
            DeleteExecutor::soft_delete_message(
                app,
                topic_id.ok_or_else(|| "message delete metadata is missing".to_string())?,
                id,
                deleted_at,
            )
            .await
        }
    }
}

async fn wait_for_operation<T: Send + 'static>(
    ctx: &AttemptContext,
    operation: impl std::future::Future<Output = Result<T, String>> + Send,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => Err("sync session cancelled".to_string()),
        result = tokio::time::timeout(ENTITY_OPERATION_TIMEOUT, operation) => result
            .map_err(|_| format!("operation timed out after {} seconds", ENTITY_OPERATION_TIMEOUT.as_secs()))
            .and_then(|result| result),
    }
}

fn required_string(payload: &Value, field: &str) -> Result<String, String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn update_data_type(payload: &Value) -> Result<SyncDataType, String> {
    match payload
        .get("dataType")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
    {
        Some(value @ (SyncDataType::Agent | SyncDataType::Group | SyncDataType::Topic)) => {
            Ok(value)
        }
        _ => Err("SYNC_ENTITY_UPDATE.dataType must be agent, group, or topic".to_string()),
    }
}

fn delete_data_type(payload: &Value) -> Result<SyncDataType, String> {
    match payload
        .get("dataType")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
    {
        Some(value) => Ok(value),
        None => Err("SYNC_DELETE_NOTIFY.dataType is missing or invalid".to_string()),
    }
}

fn topic_owner_type(payload: &Value, data_type: &SyncDataType) -> Result<String, String> {
    if *data_type != SyncDataType::Topic {
        return Ok(String::new());
    }
    match payload
        .get("ownerType")
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "agent" | "group"))
    {
        Some(value) => Ok(value.to_string()),
        None => Err("SYNC_ENTITY_UPDATE topic requires ownerType agent or group".to_string()),
    }
}

async fn protocol_failure(
    ctx: &mut AttemptContext,
    message: String,
    ids: Vec<String>,
) -> AttemptAction {
    super::commands::fail(ctx, "PROTOCOL_FRAME_INVALID", message, ids).await
}
