use super::context::DiffContext;
use super::delete_dispatch;
use super::diff_item_validation::{
    parse_validated_item, DiffAction, OwnerIdentity, ValidatedDiffItem,
};
use super::phase;
use super::topic_push::{spawn_topic_push, TopicPushRequest};
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::SyncDataType;
use futures_util::StreamExt;
use serde_json::{json, Value};

pub(crate) struct DiffBuckets {
    pub(crate) batch_pull_requests: Vec<Value>,
    pub(crate) push_topics_to_fetch: Vec<TopicPushRequest>,
    pub(crate) other_items: Vec<ValidatedDiffItem>,
}

pub(crate) async fn classify_items(
    items: Vec<Value>,
    ctx: &DiffContext,
) -> Result<DiffBuckets, String> {
    let mut buckets = DiffBuckets {
        batch_pull_requests: Vec::new(),
        push_topics_to_fetch: Vec::new(),
        other_items: Vec::new(),
    };
    for item in items {
        let item = parse_validated_item(item, &ctx.data_type)?;
        record_changed_owner(ctx, &item).await;
        match item.action {
            DiffAction::Skip => {}
            DiffAction::Pull if supports_batch_pull(&ctx.data_type) => {
                buckets.batch_pull_requests.push(json!({
                    "id": item.id,
                    "type": batch_pull_type(&ctx.data_type, item.owner.as_ref())?
                }));
            }
            DiffAction::Push if ctx.data_type == SyncDataType::Topic => {
                buckets.push_topics_to_fetch.push(TopicPushRequest {
                    id: item.id,
                    owner: item
                        .owner
                        .ok_or_else(|| "topic owner is missing".to_string())?,
                });
            }
            _ => buckets.other_items.push(item),
        }
    }
    Ok(buckets)
}

async fn record_changed_owner(ctx: &DiffContext, item: &ValidatedDiffItem) {
    if !matches!(ctx.data_type, SyncDataType::Agent | SyncDataType::Group)
        || !(matches!(item.action, DiffAction::Pull | DiffAction::Push) || item.mismatched_content)
    {
        return;
    }
    ctx.changed_owners.lock().await.insert(item.id.clone());
}

fn supports_batch_pull(data_type: &SyncDataType) -> bool {
    matches!(
        data_type,
        SyncDataType::Topic | SyncDataType::Agent | SyncDataType::Group
    )
}

fn batch_pull_type(
    data_type: &SyncDataType,
    owner: Option<&OwnerIdentity>,
) -> Result<&'static str, String> {
    match data_type {
        SyncDataType::Agent => Ok("agent"),
        SyncDataType::Group => Ok("group"),
        SyncDataType::Topic => match owner.map(|value| value.owner_type.as_str()) {
            Some("group") => Ok("group_topic"),
            Some("agent") => Ok("agent_topic"),
            _ => Err("topic pull requires a valid owner identity".to_string()),
        },
        _ => Err(format!("unsupported batch pull data type: {data_type}")),
    }
}

pub(crate) async fn dispatch_buckets(ctx: &DiffContext, buckets: DiffBuckets) {
    if !buckets.batch_pull_requests.is_empty() {
        spawn_batch_pull(ctx, buckets.batch_pull_requests).await;
    }
    if !buckets.push_topics_to_fetch.is_empty() {
        spawn_topic_push(ctx, buckets.push_topics_to_fetch).await;
    }
    if !buckets.other_items.is_empty() {
        spawn_other_items(ctx, buckets.other_items).await;
    }
}

async fn spawn_batch_pull(ctx: &DiffContext, requests: Vec<Value>) {
    let context = ctx.clone();
    ctx.task_tracker
        .spawn(async move {
            let chunk_size = match &context.data_type {
                SyncDataType::Agent | SyncDataType::Group => 50,
                SyncDataType::Topic => 1000,
                _ => 100,
            };
            for chunk in requests.chunks(chunk_size) {
                let sub_batch = chunk.to_vec();
                let count = sub_batch.len() as u32;
                let failed_topic_ids = failed_ids(&sub_batch, &context.data_type);
                if let Err(error) = PullExecutor::pull_entities_batch(
                    &context.app_handle,
                    &context.http_client,
                    &context.base_url,
                    &context.token,
                    sub_batch,
                    &context.write_queue,
                )
                .await
                {
                    send_failure(
                        &context,
                        "ENTITY_PULL_FAILED",
                        format!("Batch pull failed: {error}"),
                        failed_topic_ids,
                    );
                    return;
                }
                phase::complete_operations(&context, count);
            }
        })
        .await;
}

pub(crate) fn failed_ids(items: &[Value], data_type: &SyncDataType) -> Vec<String> {
    if *data_type != SyncDataType::Topic {
        return Vec::new();
    }
    items
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .take(8)
        .collect()
}

async fn spawn_other_items(ctx: &DiffContext, items: Vec<ValidatedDiffItem>) {
    let context = ctx.clone();
    ctx.task_tracker
        .spawn(async move {
            futures_util::stream::iter(items)
                .for_each_concurrent(15, |item| {
                    let context = context.clone();
                    async move {
                        let result = execute_item(&context, &item).await;
                        match result {
                            Ok(()) => phase::complete_operation(&context),
                            Err(error) => send_failure(
                                &context,
                                "ENTITY_OPERATION_FAILED",
                                format!(
                                    "Sync {} failed for {}: {error}",
                                    action_name(item.action),
                                    item.id
                                ),
                                failed_item_ids(&context.data_type, &item.id),
                            ),
                        }
                    }
                })
                .await;
        })
        .await;
}

async fn execute_item(ctx: &DiffContext, item: &ValidatedDiffItem) -> Result<(), String> {
    match item.action {
        DiffAction::Pull => execute_pull(ctx, &item.id).await,
        DiffAction::Push => execute_push(ctx, &item.id).await,
        DiffAction::Delete => delete_dispatch::execute_delete(ctx, item).await,
        DiffAction::PushDelete => {
            delete_dispatch::execute_delete(ctx, item).await?;
            delete_dispatch::notify_push_delete(ctx, item)
        }
        DiffAction::Skip => Ok(()),
    }
}

async fn execute_pull(ctx: &DiffContext, id: &str) -> Result<(), String> {
    match &ctx.data_type {
        SyncDataType::Avatar => {
            let (owner_type, owner_id) = split_avatar_id(id)?;
            PullExecutor::pull_avatar(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                owner_type,
                owner_id,
                &ctx.write_queue,
            )
            .await
        }
        SyncDataType::Agent => {
            PullExecutor::pull_agent(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                id,
                &ctx.write_queue,
            )
            .await
        }
        SyncDataType::Group => {
            PullExecutor::pull_group(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                id,
                &ctx.write_queue,
            )
            .await
        }
        _ => Err(format!("unsupported PULL data type: {:?}", ctx.data_type)),
    }
}

async fn execute_push(ctx: &DiffContext, id: &str) -> Result<(), String> {
    match &ctx.data_type {
        SyncDataType::Agent => {
            PushExecutor::push_agent(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                id,
            )
            .await
        }
        SyncDataType::Group => {
            PushExecutor::push_group(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                id,
            )
            .await
        }
        SyncDataType::Avatar => {
            let (owner_type, owner_id) = split_avatar_id(id)?;
            PushExecutor::push_avatar(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                owner_type,
                owner_id,
            )
            .await
        }
        _ => Err(format!("unsupported PUSH data type: {:?}", ctx.data_type)),
    }
}

fn split_avatar_id(id: &str) -> Result<(&str, &str), String> {
    let mut parts = id.splitn(2, ':');
    let owner_type = parts.next().unwrap_or_default();
    let owner_id = parts.next().unwrap_or_default();
    if crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) {
        Ok((owner_type, owner_id))
    } else {
        Err(format!("invalid avatar owner identity {id}"))
    }
}

fn action_name(action: DiffAction) -> &'static str {
    match action {
        DiffAction::Pull => "PULL",
        DiffAction::Push => "PUSH",
        DiffAction::Delete => "DELETE",
        DiffAction::PushDelete => "PUSH_DELETE",
        DiffAction::Skip => "SKIP",
    }
}

fn failed_item_ids(data_type: &SyncDataType, id: &str) -> Vec<String> {
    if *data_type == SyncDataType::Topic {
        vec![id.to_string()]
    } else {
        Vec::new()
    }
}

pub(crate) fn send_failure(
    ctx: &DiffContext,
    code: impl Into<String>,
    message: String,
    failed_topic_ids: Vec<String>,
) {
    let _ = ctx.tx_internal.send(SyncCommand::FailAttemptDetailed {
        attempt_id: ctx.attempt_id,
        code: code.into(),
        message,
        failed_topic_ids,
    });
}
