use super::delete_dispatch;
use super::manifest::ManifestDecision;
use super::topic_push::{spawn_topic_push, TopicPushRequest};
use super::{context::DiffContext, phase};
use crate::vcp_modules::sync_error::attempt_restart_code;
use crate::vcp_modules::sync_executor::{PullExecutor, PushExecutor};
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::{EntitySelector, ManifestAction, ManifestType, OwnerType};
use crate::vcp_modules::topic_types::TopicKey;
use futures_util::StreamExt;
use serde_json::to_value;

pub(crate) struct DiffBuckets {
    pub(crate) batch_pull_requests: Vec<EntitySelector>,
    pub(crate) push_topics_to_fetch: Vec<TopicPushRequest>,
    pub(crate) other_items: Vec<ManifestDecision>,
}

pub(crate) async fn classify_items(
    manifest_type: ManifestType,
    decisions: Vec<ManifestDecision>,
    ctx: &DiffContext,
) -> Result<DiffBuckets, String> {
    let mut buckets = DiffBuckets {
        batch_pull_requests: Vec::new(),
        push_topics_to_fetch: Vec::new(),
        other_items: Vec::new(),
    };
    for decision in decisions {
        record_changed_owner(ctx, &decision).await;
        match decision.action() {
            ManifestAction::Skip => {}
            ManifestAction::Pull => match &decision {
                ManifestDecision::Owner(item) => buckets
                    .batch_pull_requests
                    .push(EntitySelector::owner(item.owner_type, &item.owner_id)),
                ManifestDecision::Topic(item) => {
                    let key =
                        TopicKey::new(item.owner_type.as_str(), &item.owner_id, &item.topic_id);
                    buckets
                        .batch_pull_requests
                        .push(EntitySelector::topic(&key)?);
                }
                ManifestDecision::Avatar(_) => buckets.other_items.push(decision),
            },
            ManifestAction::Push => match &decision {
                ManifestDecision::Topic(item) => {
                    buckets
                        .push_topics_to_fetch
                        .push(TopicPushRequest::new(TopicKey::new(
                            item.owner_type.as_str(),
                            &item.owner_id,
                            &item.topic_id,
                        )))
                }
                ManifestDecision::Owner(_) | ManifestDecision::Avatar(_) => {
                    buckets.other_items.push(decision)
                }
            },
            ManifestAction::PullDelete | ManifestAction::PushDelete => {
                buckets.other_items.push(decision)
            }
        }
    }
    if manifest_type == ManifestType::Topic
        && buckets
            .other_items
            .iter()
            .any(|item| !matches!(item, ManifestDecision::Topic(_)))
    {
        return Err("Topic manifest decision contains a non-topic identity".to_string());
    }
    Ok(buckets)
}

async fn record_changed_owner(ctx: &DiffContext, decision: &ManifestDecision) {
    let Some(owner) = decision.owner_key() else {
        return;
    };
    let is_owner_content_mismatch = matches!(
        decision,
        ManifestDecision::Owner(item) if item.content_hash_mismatch
    );
    if matches!(
        decision.action(),
        ManifestAction::Pull | ManifestAction::Push
    ) || is_owner_content_mismatch
    {
        ctx.changed_owners.lock().await.insert(owner);
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

async fn spawn_batch_pull(ctx: &DiffContext, requests: Vec<EntitySelector>) {
    let context = ctx.clone();
    ctx.task_tracker
        .spawn(async move {
            let chunk_size = match context.manifest_type {
                crate::vcp_modules::sync_types::ManifestType::Owner
                | crate::vcp_modules::sync_types::ManifestType::Avatar => 50,
                crate::vcp_modules::sync_types::ManifestType::Topic => 1000,
            };
            for chunk in requests.chunks(chunk_size) {
                let sub_batch = match chunk.iter().map(to_value).collect::<Result<Vec<_>, _>>() {
                    Ok(value) => value,
                    Err(error) => {
                        send_failure(
                            &context,
                            "ENTITY_PULL_REQUEST_INVALID",
                            format!("Entity selector serialization failed: {error}"),
                            failed_ids(chunk),
                        );
                        return;
                    }
                };
                let count = sub_batch.len() as u32;
                if let Err(error) = PullExecutor::pull_entities_batch(
                    &context.app_handle,
                    &context.http_client,
                    &context.base_url,
                    &context.token,
                    sub_batch,
                    &context.write_queue,
                    context.owner_config_baselines.lock().await.clone(),
                )
                .await
                {
                    let code = entity_pull_failure_code(&error);
                    send_failure(
                        &context,
                        code,
                        format!("Batch pull failed: {error}"),
                        failed_ids(chunk),
                    );
                    return;
                }
                if let Err(error) = context.write_queue.flush().await {
                    let code = entity_pull_failure_code(&error);
                    send_failure(
                        &context,
                        code,
                        format!("Batch pull write drain failed: {error}"),
                        failed_ids(chunk),
                    );
                    return;
                }
                phase::complete_operations(&context, count);
            }
        })
        .await;
}

pub(crate) fn entity_pull_failure_code(error: &str) -> String {
    attempt_restart_code("ENTITY_PULL_FAILED", error)
        .unwrap_or_else(|| "ENTITY_PULL_FAILED".to_string())
}

pub(crate) fn failed_ids(items: &[EntitySelector]) -> Vec<String> {
    items
        .iter()
        .filter_map(EntitySelector::topic_id)
        .map(str::to_string)
        .take(8)
        .collect()
}

async fn spawn_other_items(ctx: &DiffContext, items: Vec<ManifestDecision>) {
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
                                    item.action(),
                                    item.display_id()
                                ),
                                failed_decision_ids(&item),
                            ),
                        }
                    }
                })
                .await;
        })
        .await;
}

async fn execute_item(ctx: &DiffContext, item: &ManifestDecision) -> Result<(), String> {
    match item.action() {
        ManifestAction::Pull => execute_pull(ctx, item).await,
        ManifestAction::Push => execute_push(ctx, item).await,
        ManifestAction::PullDelete | ManifestAction::PushDelete => {
            delete_dispatch::execute_delete(ctx, item).await?;
            if item.action() == ManifestAction::PushDelete {
                delete_dispatch::notify_push_delete(ctx, item)?;
            }
            Ok(())
        }
        ManifestAction::Skip => Ok(()),
    }
}

async fn execute_pull(ctx: &DiffContext, item: &ManifestDecision) -> Result<(), String> {
    match item {
        ManifestDecision::Avatar(avatar) => {
            PullExecutor::pull_avatar(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                avatar.owner_type.as_str(),
                &avatar.owner_id,
                &ctx.write_queue,
            )
            .await
        }
        _ => Err(format!("unsupported PULL identity {}", item.identity())),
    }
}

async fn execute_push(ctx: &DiffContext, item: &ManifestDecision) -> Result<(), String> {
    match item {
        ManifestDecision::Owner(owner) => match owner.owner_type {
            OwnerType::Agent => {
                PushExecutor::push_agent(
                    &ctx.app_handle,
                    &ctx.http_client,
                    &ctx.base_url,
                    &ctx.token,
                    &owner.owner_id,
                )
                .await
            }
            OwnerType::Group => {
                PushExecutor::push_group(
                    &ctx.app_handle,
                    &ctx.http_client,
                    &ctx.base_url,
                    &ctx.token,
                    &owner.owner_id,
                )
                .await
            }
        },
        ManifestDecision::Avatar(avatar) => {
            PushExecutor::push_avatar(
                &ctx.app_handle,
                &ctx.http_client,
                &ctx.base_url,
                &ctx.token,
                avatar.owner_type.as_str(),
                &avatar.owner_id,
            )
            .await
        }
        ManifestDecision::Topic(_) => Err("topic PUSH must be dispatched as a topic batch".into()),
    }
}

fn failed_decision_ids(decision: &ManifestDecision) -> Vec<String> {
    decision
        .topic_key()
        .map(|key| vec![key.topic_id])
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::entity_pull_failure_code;
    use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};

    #[test]
    fn entity_pull_drain_maps_agent_and_group_cas_to_restartable_code() {
        for owner_type in ["Agent", "Group"] {
            let error = format!(
                "rusqlite execution error: {}",
                encode_local_sync_error(
                    "SYNC_SNAPSHOT_STALE",
                    SyncErrorStage::OwnerMetadata,
                    &format!("local {owner_type} changed"),
                    Vec::new(),
                )
            );
            assert_eq!(entity_pull_failure_code(&error), "SYNC_SNAPSHOT_STALE");
        }
    }

    #[test]
    fn entity_pull_only_reports_generic_failures_as_non_restartable() {
        assert_eq!(
            entity_pull_failure_code("HTTP 400 without a Wire error"),
            "ENTITY_PULL_FAILED"
        );
        assert_eq!(
            entity_pull_failure_code(
                "rusqlite execution error: SYNC_SNAPSHOT_STALE: local Agent changed"
            ),
            "ENTITY_PULL_FAILED"
        );
    }
}
