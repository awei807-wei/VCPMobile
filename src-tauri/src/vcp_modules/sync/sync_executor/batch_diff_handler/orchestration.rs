use super::codec::MAX_PHASE3_TOPICS;
use super::context::Phase3Context;
use super::decision::{parse_topic_decision, TopicDecision};
use super::error::Phase3ProtocolError;
use super::outcome::{
    validate_phase3_result_topics, validate_topic_batch_outcomes, TopicBatchOutcome,
};
use crate::vcp_modules::sync_executor::pull_executor::PullBatchContext;
use crate::vcp_modules::sync_executor::{
    BatchPullResult, PullExecutor, PullProgressContext, PushExecutor,
};
use crate::vcp_modules::sync_service::SyncCommand;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::atomic::Ordering;

struct BatchPlan {
    push_topic_ids: Vec<String>,
    pull_batch: Vec<(String, Vec<String>)>,
    idle_topic_ids: Vec<String>,
    all_topic_ids: HashSet<String>,
}

pub(crate) async fn handle_diff_batch(
    ctx: &Phase3Context,
    payload: &Value,
) -> Result<(), Phase3ProtocolError> {
    let results = phase3_results(payload)?;
    validate_expected_topics(ctx, results).await?;
    let plan = build_plan(results)?;
    complete_idle_topics(ctx, &plan.idle_topic_ids).await;
    mark_active_topics_modified(ctx, &plan.all_topic_ids).await;
    if !plan.push_topic_ids.is_empty() {
        run_push(ctx, &plan.push_topic_ids).await?;
    }
    if !plan.pull_batch.is_empty() {
        run_pull(ctx, &plan.pull_batch).await?;
    }
    complete_active_topics(ctx, &plan.all_topic_ids).await;
    log::info!(
        "[SyncService] Phase 3 batch done: push={} pull={}",
        plan.push_topic_ids.len(),
        plan.pull_batch.len()
    );
    send_next_batch(ctx).await;
    Ok(())
}

fn phase3_results(payload: &Value) -> Result<&Map<String, Value>, Phase3ProtocolError> {
    let object = payload.as_object().ok_or_else(|| {
        Phase3ProtocolError::new("PHASE3_FRAME_INVALID", "Phase 3 response must be an object")
    })?;
    if object.len() != 2
        || !object
            .keys()
            .all(|key| matches!(key.as_str(), "type" | "results"))
        || object.get("type").and_then(Value::as_str) != Some("SYNC_DIFF_RESULTS_BATCH")
    {
        return Err(Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            "Phase 3 batch frame must contain exactly type/results",
        ));
    }
    let results = object
        .get("results")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            Phase3ProtocolError::new(
                "PHASE3_FRAME_INVALID",
                "Phase 3 response is missing results",
            )
        })?;
    if results.len() > MAX_PHASE3_TOPICS {
        return Err(Phase3ProtocolError::new(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!("Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"),
        ));
    }
    Ok(results)
}

async fn validate_expected_topics(
    ctx: &Phase3Context,
    results: &Map<String, Value>,
) -> Result<(), Phase3ProtocolError> {
    let expected = ctx.expected_batch_topics.lock().await;
    validate_phase3_result_topics(&expected, results).map_err(|message| {
        let mut failed_topic_ids = expected.iter().cloned().collect::<Vec<_>>();
        failed_topic_ids.sort();
        failed_topic_ids.truncate(8);
        Phase3ProtocolError {
            code: "PHASE3_TOPIC_MISMATCH".to_string(),
            message,
            failed_topic_ids,
        }
    })
}

fn build_plan(results: &Map<String, Value>) -> Result<BatchPlan, Phase3ProtocolError> {
    let mut plan = BatchPlan {
        push_topic_ids: Vec::new(),
        pull_batch: Vec::new(),
        idle_topic_ids: Vec::new(),
        all_topic_ids: HashSet::new(),
    };
    let mut total_pull_messages = 0usize;
    for (topic_id, result) in results {
        let decision = parse_topic_decision(topic_id, result)?;
        add_decision(&mut plan, topic_id, decision, &mut total_pull_messages)?;
    }
    Ok(plan)
}

fn add_decision(
    plan: &mut BatchPlan,
    topic_id: &str,
    decision: TopicDecision,
    total_pull_messages: &mut usize,
) -> Result<(), Phase3ProtocolError> {
    *total_pull_messages = total_pull_messages
        .checked_add(decision.to_pull.len())
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_BUDGET_EXCEEDED",
                "Phase 3 toPull message count overflow",
                topic_id,
            )
        })?;
    if *total_pull_messages > super::codec::MAX_PHASE3_MESSAGES {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!(
                "Phase 3 response exceeds {} toPull message budget",
                super::codec::MAX_PHASE3_MESSAGES
            ),
            topic_id,
        ));
    }
    if !decision.to_push && decision.to_pull.is_empty() {
        plan.idle_topic_ids.push(topic_id.to_string());
        return Ok(());
    }
    plan.all_topic_ids.insert(topic_id.to_string());
    if decision.to_push {
        plan.push_topic_ids.push(topic_id.to_string());
    }
    if !decision.to_pull.is_empty() {
        plan.pull_batch
            .push((topic_id.to_string(), decision.to_pull));
    }
    Ok(())
}

async fn complete_idle_topics(ctx: &Phase3Context, topic_ids: &[String]) {
    for topic_id in topic_ids {
        ctx.tracker.mark_modified(topic_id).await;
        ctx.tracker
            .mark_completed(
                topic_id,
                &ctx.logger,
                &ctx.tx_internal,
                &ctx.app_handle,
                true,
            )
            .await;
    }
}

async fn mark_active_topics_modified(ctx: &Phase3Context, topic_ids: &HashSet<String>) {
    for topic_id in topic_ids {
        ctx.tracker.mark_modified(topic_id).await;
    }
}

async fn complete_active_topics(ctx: &Phase3Context, topic_ids: &HashSet<String>) {
    for topic_id in topic_ids {
        ctx.tracker
            .mark_completed(
                topic_id,
                &ctx.logger,
                &ctx.tx_internal,
                &ctx.app_handle,
                false,
            )
            .await;
    }
}

async fn run_push(ctx: &Phase3Context, topic_ids: &[String]) -> Result<(), Phase3ProtocolError> {
    let result = PushExecutor::push_messages_batch(
        &ctx.app_handle,
        &ctx.http_client,
        &ctx.base_url,
        &ctx.token,
        topic_ids,
        ctx.uploaded_hashes.clone(),
    )
    .await
    .map(|results| {
        results
            .into_iter()
            .map(|result| TopicBatchOutcome {
                topic_id: result.topic_id,
                success: result.success,
                error: result.error,
            })
            .collect()
    });
    apply_batch_outcomes(ctx, "push", topic_ids, result).await
}

async fn run_pull(
    ctx: &Phase3Context,
    pull_batch: &[(String, Vec<String>)],
) -> Result<(), Phase3ProtocolError> {
    let topic_ids = pull_batch
        .iter()
        .map(|(topic_id, _)| topic_id.clone())
        .collect::<Vec<_>>();
    let progress = pull_progress(ctx).await;
    let result = PullExecutor::pull_messages_batch(PullBatchContext {
        app: &ctx.app_handle,
        client: &ctx.http_client,
        http_url: &ctx.base_url,
        sync_token: &ctx.token,
        requests: pull_batch,
        write_queue: &ctx.write_queue,
        prerender_enabled: ctx.prerender_enabled,
        progress: Some(progress),
    })
    .await
    .map(|results| {
        ctx.tracker.add_legacy_attachment_warnings(
            results
                .iter()
                .map(|result| result.legacy_attachment_warnings)
                .sum(),
        );
        results
            .into_iter()
            .map(|result: BatchPullResult| TopicBatchOutcome {
                topic_id: result.topic_id,
                success: result.success,
                error: result.error,
            })
            .collect()
    });
    apply_batch_outcomes(ctx, "pull", &topic_ids, result).await
}

async fn pull_progress(ctx: &Phase3Context) -> PullProgressContext {
    PullProgressContext {
        session_id: ctx.tracker.session_id,
        base_completed: ctx.tracker.completed.lock().await.len(),
        total: ctx.tracker.total.load(Ordering::SeqCst),
        failed: ctx.tracker.failed.lock().await.len(),
        legacy_attachment_warnings: ctx
            .tracker
            .legacy_attachment_warnings
            .load(Ordering::SeqCst),
    }
}

async fn apply_batch_outcomes(
    ctx: &Phase3Context,
    operation: &str,
    expected: &[String],
    result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<(), Phase3ProtocolError> {
    match validate_topic_batch_outcomes(operation, expected, result) {
        Ok(successful) => {
            for topic_id in successful {
                ctx.tracker.mark_modified(&topic_id).await;
            }
            Ok(())
        }
        Err(message) => {
            for topic_id in expected {
                ctx.tracker.mark_failed(topic_id).await;
            }
            let mut failed_topic_ids = expected.to_vec();
            failed_topic_ids.sort();
            failed_topic_ids.truncate(8);
            Err(Phase3ProtocolError {
                code: format!("PHASE3_{}_FAILED", operation.to_uppercase()),
                message,
                failed_topic_ids,
            })
        }
    }
}

async fn send_next_batch(ctx: &Phase3Context) {
    let mut pending = ctx.pending_diff_batches.lock().await;
    let Some(next_batch) = pending.pop_front() else {
        return;
    };
    log::debug!(
        "[SyncService] Sending next diff batch, {} remaining",
        pending.len()
    );
    let message = json!({
        "type": "SYNC_MESSAGE_DIFF_BATCH",
        "topics": next_batch,
    });
    let mut expected = ctx.expected_batch_topics.lock().await;
    *expected = message["topics"]
        .as_object()
        .map(|topics| topics.keys().cloned().collect())
        .unwrap_or_default();
    drop(expected);
    let _ = ctx.tx_internal.send(SyncCommand::SendWsMessage {
        attempt_id: ctx.attempt_id,
        value: message,
    });
}
