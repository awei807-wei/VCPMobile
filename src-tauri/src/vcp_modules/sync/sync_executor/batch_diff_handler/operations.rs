use super::context::BatchContext;
use super::error::Phase3ProtocolError;
use super::outcome::{validate_topic_batch_outcomes, TopicBatchFailure, TopicBatchOutcome};
use super::plan::BatchPlan;
use crate::vcp_modules::db_write_queue::SNAPSHOT_STALE_MARKER;
use crate::vcp_modules::sync_error::decode_wire_sync_error;
use crate::vcp_modules::sync_executor::{
    BatchPullResult, DeleteExecutor, MessageBatchPullRequest, PullExecutor, PullProgressContext,
    PushExecutor,
};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, MessageDiffResultFrame};
use crate::vcp_modules::topic_types::TopicKey;
use std::sync::atomic::Ordering;

/// Validate, execute and finalize one Wire 1.4 message-diff batch.
pub(crate) async fn handle_batch(
    ctx: BatchContext<'_>,
    frame: MessageDiffResultFrame,
) -> Result<(), Phase3ProtocolError> {
    let keyed = {
        let expected = ctx.expected_topics.lock().await;
        super::plan::validate_phase3_result_topics(&expected, &frame.results)
            .map_err(|message| topic_mismatch(message, &expected))?
    };
    let plan = super::plan::build_plan(keyed)?;
    complete_idle_topics(&ctx, &plan).await;
    mark_active_topics_modified(&ctx, &plan).await;
    apply_operations(&ctx, &plan).await?;
    complete_active_topics(&ctx, &plan).await;
    log_batch_summary(&plan);
    super::next_batch::send_next_batch(ctx).await;
    Ok(())
}

fn topic_mismatch(
    message: String,
    expected: &std::collections::HashSet<TopicKey>,
) -> Phase3ProtocolError {
    let mut failed_topic_ids = expected
        .iter()
        .map(|topic| topic.topic_id.clone())
        .collect::<Vec<_>>();
    failed_topic_ids.sort();
    failed_topic_ids.truncate(8);
    Phase3ProtocolError {
        code: "PHASE3_TOPIC_MISMATCH".to_string(),
        message,
        failed_topic_ids,
    }
}

async fn complete_idle_topics(ctx: &BatchContext<'_>, plan: &BatchPlan) {
    for topic in &plan.idle_topics {
        ctx.tracker.mark_modified(topic).await;
        ctx.tracker
            .mark_completed(topic, ctx.logger, ctx.tx, ctx.app, true)
            .await;
    }
}

async fn mark_active_topics_modified(ctx: &BatchContext<'_>, plan: &BatchPlan) {
    for topic in &plan.active_topics {
        ctx.tracker.mark_modified(topic).await;
    }
}

async fn complete_active_topics(ctx: &BatchContext<'_>, plan: &BatchPlan) {
    for topic in &plan.active_topics {
        ctx.tracker
            .mark_completed(topic, ctx.logger, ctx.tx, ctx.app, false)
            .await;
    }
}

async fn apply_operations(
    ctx: &BatchContext<'_>,
    plan: &BatchPlan,
) -> Result<(), Phase3ProtocolError> {
    if !plan.delete_batch.is_empty() {
        apply_deletes(ctx, &plan.delete_batch).await?;
    }
    if !plan.pull_batch.is_empty() {
        run_pull(ctx, &plan.pull_batch).await?;
        flush_pull(ctx, &plan.pull_batch).await?;
    }
    if !plan.push_topics.is_empty() {
        run_push(ctx, &plan.push_topics).await?;
    }
    Ok(())
}

async fn apply_deletes(
    ctx: &BatchContext<'_>,
    delete_batch: &[(TopicKey, Vec<MessageDeleteDecision>)],
) -> Result<(), Phase3ProtocolError> {
    let snapshots = ctx.expected_states.lock().await.clone();
    for (topic, tombstones) in delete_batch {
        let snapshot = snapshots.get(topic).ok_or_else(|| {
            snapshot_stale_error(topic, "Phase 3 local snapshot is missing a deleted topic")
        })?;
        let expected_states = tombstones
            .iter()
            .map(|item| (item.msg_id.clone(), snapshot.get(&item.msg_id).cloned()))
            .collect();
        if let Err(error) =
            DeleteExecutor::soft_delete_messages(ctx.app, topic, tombstones, &expected_states).await
        {
            ctx.tracker.mark_failed(topic).await;
            if is_snapshot_stale_wire_error(&error) {
                return Err(snapshot_stale_error(topic, &error));
            }
            return Err(Phase3ProtocolError::for_topic(
                "SYNC_DELETE_FAILED",
                format!(
                    "Failed to apply {} Desktop message tombstones for {}: {error}",
                    tombstones.len(),
                    topic.topic_id
                ),
                &topic.topic_id,
            ));
        }
        ctx.tracker.mark_modified(topic).await;
    }
    Ok(())
}

fn is_snapshot_stale_wire_error(error: &str) -> bool {
    decode_wire_sync_error(error).is_some_and(|wire| wire.code == SNAPSHOT_STALE_MARKER)
}

fn snapshot_stale_error(topic: &TopicKey, detail: &str) -> Phase3ProtocolError {
    Phase3ProtocolError::for_topic(
        SNAPSHOT_STALE_MARKER,
        format!(
            "Local data changed after the Phase 3 snapshot for {}: {detail}",
            topic.topic_id
        ),
        &topic.topic_id,
    )
}

async fn run_pull(
    ctx: &BatchContext<'_>,
    pull_batch: &[(TopicKey, Vec<String>)],
) -> Result<(), Phase3ProtocolError> {
    let expected_local_states = ctx.expected_states.lock().await.clone();
    let expected = pull_batch
        .iter()
        .map(|(topic, _)| topic.clone())
        .collect::<Vec<_>>();
    let result = PullExecutor::pull_messages_batch_with_progress(MessageBatchPullRequest {
        app: ctx.app,
        client: ctx.client,
        http_url: ctx.base_url,
        sync_token: ctx.token,
        requests: pull_batch,
        expected_local_states: Some(&expected_local_states),
        write_queue: ctx.write_queue,
        prerender_enabled: ctx.prerender_enabled,
        progress: Some(pull_progress_context(ctx).await),
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
                topic: result.topic,
                success: result.success,
                error: result.error,
            })
            .collect()
    });
    apply_outcomes(ctx, "pull", &expected, result).await
}

async fn pull_progress_context(ctx: &BatchContext<'_>) -> PullProgressContext {
    PullProgressContext {
        session_id: ctx.tracker.session_id,
        attempt_id: ctx.attempt_id,
        base_completed: ctx.tracker.completed.lock().await.len(),
        total: ctx.tracker.total.load(Ordering::SeqCst),
        failed: ctx.tracker.failed.lock().await.len(),
        legacy_attachment_warnings: ctx
            .tracker
            .legacy_attachment_warnings
            .load(Ordering::SeqCst),
    }
}

async fn flush_pull(
    ctx: &BatchContext<'_>,
    pull_batch: &[(TopicKey, Vec<String>)],
) -> Result<(), Phase3ProtocolError> {
    if let Err(error) = ctx.write_queue.flush().await {
        let topics = pull_batch
            .iter()
            .map(|(topic, _)| topic.clone())
            .collect::<Vec<_>>();
        mark_failed(ctx, &topics).await;
        return Err(operation_error(
            "PHASE3_PULL_FAILED",
            format!("Phase 3 pull write drain failed before merged push: {error}"),
            &topics,
        ));
    }
    Ok(())
}

async fn run_push(ctx: &BatchContext<'_>, topics: &[TopicKey]) -> Result<(), Phase3ProtocolError> {
    let result =
        PushExecutor::push_messages_batch(ctx.app, ctx.client, ctx.base_url, ctx.token, topics)
            .await
            .map(|results| {
                results
                    .into_iter()
                    .map(|result| TopicBatchOutcome {
                        topic: result.topic,
                        success: result.success,
                        error: result.error,
                    })
                    .collect()
            });
    apply_outcomes(ctx, "push", topics, result).await
}

async fn apply_outcomes(
    ctx: &BatchContext<'_>,
    operation: &str,
    expected: &[TopicKey],
    result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<(), Phase3ProtocolError> {
    match validate_topic_batch_outcomes(operation, expected, result) {
        Ok(successful) => {
            for topic in successful {
                ctx.tracker.mark_modified(&topic).await;
            }
            Ok(())
        }
        Err(failure) => {
            mark_failed(ctx, expected).await;
            Err(operation_failure(operation, failure, expected))
        }
    }
}

async fn mark_failed(ctx: &BatchContext<'_>, topics: &[TopicKey]) {
    for topic in topics {
        ctx.tracker.mark_failed(topic).await;
    }
}

fn operation_failure(
    operation: &str,
    failure: TopicBatchFailure,
    topics: &[TopicKey],
) -> Phase3ProtocolError {
    let code = failure
        .restart_code
        .unwrap_or_else(|| format!("PHASE3_{}_FAILED", operation.to_uppercase()));
    operation_error(&code, failure.message, topics)
}

fn operation_error(code: &str, message: String, topics: &[TopicKey]) -> Phase3ProtocolError {
    let mut failed_topic_ids = topics
        .iter()
        .map(|topic| topic.topic_id.clone())
        .collect::<Vec<_>>();
    failed_topic_ids.sort();
    failed_topic_ids.truncate(8);
    Phase3ProtocolError {
        code: code.to_string(),
        message,
        failed_topic_ids,
    }
}

fn log_batch_summary(plan: &BatchPlan) {
    log::info!(
        "[SyncService] Phase 3 batch done: push={} pull={} delete={}",
        plan.push_topics.len(),
        plan.pull_batch.len(),
        plan.delete_batch
            .iter()
            .map(|(_, tombstones)| tombstones.len())
            .sum::<usize>()
    );
}

#[cfg(test)]
mod tests {
    use super::is_snapshot_stale_wire_error;
    use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};

    #[test]
    fn delete_error_classification_requires_a_strict_snapshot_error_envelope() {
        assert!(!is_snapshot_stale_wire_error(
            "rusqlite execution error: SYNC_SNAPSHOT_STALE: local message changed"
        ));
        assert!(!is_snapshot_stale_wire_error(
            r#"SYNC_WIRE_ERROR:{"code":"SYNC_SNAPSHOT_STALE","message":"changed"}"#
        ));
        assert!(!is_snapshot_stale_wire_error("database is locked"));

        let normal_wire = encode_local_sync_error(
            "SYNC_DB_QUERY_FAILED",
            SyncErrorStage::Messages,
            "normal SQLite failure mentions SYNC_SNAPSHOT_STALE",
            Vec::new(),
        );
        assert!(!is_snapshot_stale_wire_error(&normal_wire));

        let stale_wire = encode_local_sync_error(
            "SYNC_SNAPSHOT_STALE",
            SyncErrorStage::Messages,
            "local message changed",
            Vec::new(),
        );
        assert!(is_snapshot_stale_wire_error(&stale_wire));
    }
}
