use super::attempt::{AttemptAction, AttemptContext};
use super::batching::{build_diff_batches, Phase3DiffBatch};
use super::protocol::{
    enforce_manifest_response_deadline, enforce_topic_hash_response_deadline,
    send_ws_with_deadline, PHASE3_WATCHDOG_STUCK_TICKS, PHASE3_WATCHDOG_TICK,
    PHASE_RESPONSE_TIMEOUT,
};
use super::types::SyncCommand;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message};
use crate::vcp_modules::sync_types::{
    ManifestRequestFrame, ManifestType, OwnerType, SyncPhase, TopicDiffRequestFrame, TopicDiffState,
};
use crate::vcp_modules::topic_types::TopicKey;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use tokio_tungstenite::tungstenite::protocol::Message;

pub(crate) async fn handle_pipeline_command(
    ctx: &mut AttemptContext,
    command: crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand,
) -> AttemptAction {
    use crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand;
    match command {
        PipelineCommand::StartTopicMetadata => start_topic_metadata(ctx).await,
        PipelineCommand::StartTopicValidation => start_topic_validation(ctx).await,
        PipelineCommand::StartMessages => start_messages(ctx).await,
        PipelineCommand::Finalize => {
            emit_sync_log(
                ctx,
                "info",
                "Local finalization complete; waiting for desktop acknowledgement",
            );
            AttemptAction::Continue
        }
    }
}

async fn start_topic_metadata(ctx: &mut AttemptContext) -> AttemptAction {
    let owners = ctx
        .changed_owners
        .lock()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    if owners.is_empty() {
        clear_manifest_expectation(ctx);
        let _ = ctx.tx.send(SyncCommand::StartTopicValidation {
            attempt_id: ctx.attempt_id,
        });
        return AttemptAction::Continue;
    }

    let db = ctx.app.state::<DbState>();
    let manifest = match Phase1Metadata::build_targeted_topic_manifest(&db.pool, &owners).await {
        Ok(manifest) if manifest.manifest_type() == ManifestType::Topic => manifest,
        Ok(_) => {
            return fail_attempt(
                ctx,
                "TOPIC_MANIFEST_INVALID",
                "Targeted topic manifest returned an unexpected manifest type".to_string(),
            )
            .await;
        }
        Err(error) => return fail_attempt(ctx, "TOPIC_MANIFEST_DB_FAILED", error).await,
    };
    ctx.manifest_phase.store(3, Ordering::SeqCst);
    set_manifest_expectation(ctx, HashSet::from([ManifestType::Topic]));
    ctx.pending_tasks.store(0, Ordering::SeqCst);
    ctx.total_tasks.store(0, Ordering::SeqCst);

    if send_phase_start(ctx, SyncPhase::TopicMetadata).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    let frame = ManifestRequestFrame::new(manifest);
    if send_frame(ctx, &frame).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    schedule_manifest_deadline(ctx, 3).await;
    AttemptAction::Continue
}

async fn start_topic_validation(ctx: &mut AttemptContext) -> AttemptAction {
    emit_sync_log(ctx, "info", "=== Phase 2.5: Validating Topic Hashes ===");
    let (topics, expected_topics) = match collect_topic_states(ctx).await {
        Ok(value) => value,
        Err(action) => return action,
    };
    let response_overlap = {
        let mut expected = ctx.expected_topic_hash_results.lock().await;
        if expected.is_some() {
            true
        } else {
            *expected = Some(expected_topics);
            false
        }
    };
    if response_overlap {
        return fail_attempt(
            ctx,
            "TOPIC_HASH_RESPONSE_OVERLAP",
            "A topic diff response is already pending".to_string(),
        )
        .await;
    }
    let frame = TopicDiffRequestFrame::new(topics);
    if let Err(error) = frame.validate() {
        return fail_attempt(ctx, "TOPIC_HASH_STATE_INVALID", error).await;
    }
    if send_frame(ctx, &frame).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    ctx.task_tracker
        .spawn(enforce_topic_hash_response_deadline(
            ctx.expected_topic_hash_results.clone(),
            ctx.manifest_phase.clone(),
            3,
            ctx.tx.clone(),
            ctx.attempt_id,
            PHASE_RESPONSE_TIMEOUT,
        ))
        .await;
    AttemptAction::Continue
}

async fn collect_topic_states(
    ctx: &mut AttemptContext,
) -> Result<(Vec<TopicDiffState>, HashSet<TopicKey>), AttemptAction> {
    let owners = ctx
        .changed_owners
        .lock()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let db = ctx.app.state::<DbState>();
    let hashes = Phase3Message::get_targeted_topic_hashes(&db.pool, &owners)
        .await
        .map_err(|error| queue_failure(ctx, "TOPIC_HASH_DB_FAILED", error))?;
    if hashes.len() > super::protocol::MAX_SYNC_TOPICS {
        return Err(queue_failure(
            ctx,
            "TOPIC_HASH_BUDGET_EXCEEDED",
            "Topic diff exceeds the configured topic budget".to_string(),
        ));
    }

    let mut expected = HashSet::with_capacity(hashes.len());
    let mut topics = Vec::with_capacity(hashes.len());
    for (key, state) in hashes {
        let owner_type = OwnerType::try_from(key.owner_type.as_str()).map_err(|_| {
            queue_failure(
                ctx,
                "TOPIC_HASH_STATE_INVALID",
                format!("Topic {} has invalid ownerType", key.topic_id),
            )
        })?;
        expected.insert(key.clone());
        topics.push(TopicDiffState {
            owner_type,
            owner_id: key.owner_id,
            topic_id: key.topic_id,
            config_hash: state.config_hash,
            content_hash: state.content_hash,
        });
    }
    Ok((topics, expected))
}

fn queue_failure(ctx: &AttemptContext, code: &'static str, message: String) -> AttemptAction {
    let _ = ctx.tx.send(SyncCommand::FailAttempt {
        attempt_id: ctx.attempt_id,
        code,
        message,
    });
    AttemptAction::Continue
}

async fn start_messages(ctx: &mut AttemptContext) -> AttemptAction {
    emit_sync_log(ctx, "info", "=== Phase 3: Messages ===");
    ctx.message_phase_barrier.reset();
    if send_phase_start(ctx, SyncPhase::Messages).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    let changed_topics = ctx.changed_topics.lock().await.clone();
    if changed_topics.is_empty() {
        let _ = ctx.tx.send(SyncCommand::Finalize {
            attempt_id: ctx.attempt_id,
        });
        return AttemptAction::Continue;
    }
    let db = ctx.app.state::<DbState>();
    let states = match Phase3Message::get_topic_message_hashes(&db.pool, &changed_topics).await {
        Ok(value) => value,
        Err(error) => return fail_attempt(ctx, "PHASE3_HASH_PREP_FAILED", error).await,
    };
    prepare_phase3_tracker(ctx, states.len()).await;
    let first = match prepare_phase3_batches(ctx, states).await {
        Ok(batch) => batch,
        Err(error) => return fail_attempt(ctx, "PHASE3_DIFF_BUDGET_EXCEEDED", error).await,
    };
    if send_phase3_batch(ctx, first).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    start_phase3_watchdog(ctx).await;
    AttemptAction::Continue
}

async fn prepare_phase3_tracker(ctx: &AttemptContext, topic_count: usize) {
    ctx.pending_topics
        .total
        .store(topic_count, Ordering::SeqCst);
    ctx.pending_topics.completed.lock().await.clear();
    ctx.pending_topics.modified.lock().await.clear();
    ctx.pending_topics.failed.lock().await.clear();
    ctx.pending_topics
        .legacy_attachment_warnings
        .store(0, Ordering::SeqCst);
    let _ = ctx.app.emit(
        "vcp-sync-progress",
        serde_json::json!({
            "sessionId": ctx.session_id,
            "attemptId": ctx.attempt_id,
            "phase": "messages",
            "total": topic_count,
            "completed": 0,
            "successfulTopics": 0,
            "totalTopics": topic_count,
            "failedTopics": 0,
            "legacyAttachmentWarnings": 0,
        }),
    );
}

async fn prepare_phase3_batches(
    ctx: &AttemptContext,
    states: std::collections::HashMap<
        TopicKey,
        crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState,
    >,
) -> Result<Phase3DiffBatch, String> {
    let mut batches = build_diff_batches(states)?;
    let first = batches
        .pop_front()
        .ok_or_else(|| "Phase 3 produced no request batch for changed topics".to_string())?;
    *ctx.pending_batches.lock().await = batches;
    Ok(first)
}

async fn send_phase3_batch(ctx: &mut AttemptContext, batch: Phase3DiffBatch) -> AttemptAction {
    *ctx.expected_phase3_states.lock().await = batch.message_snapshots();
    *ctx.expected_phase3_batch.lock().await = batch.keys;
    let frame = crate::vcp_modules::sync_types::MessageDiffRequestFrame::new(batch.topics);
    if let Err(error) = frame.validate() {
        return fail_attempt(ctx, "PHASE3_DIFF_BUDGET_EXCEEDED", error).await;
    }
    send_frame(ctx, &frame).await
}

async fn start_phase3_watchdog(ctx: &AttemptContext) {
    let tracker = ctx.pending_topics.clone();
    let tx = ctx.tx.clone();
    let attempt_id = ctx.attempt_id;
    ctx.task_tracker
        .spawn(async move {
            let mut last = 0;
            let mut stuck = 0;
            loop {
                tokio::time::sleep(PHASE3_WATCHDOG_TICK).await;
                let completed = tracker.completed.lock().await.len();
                let total = tracker.total.load(Ordering::SeqCst);
                if completed >= total {
                    break;
                }
                if completed == last {
                    stuck += 1;
                } else {
                    last = completed;
                    stuck = 0;
                }
                if stuck >= PHASE3_WATCHDOG_STUCK_TICKS {
                    let _ = tx.send(SyncCommand::FailAttempt {
                        attempt_id,
                        code: "PHASE3_RESPONSE_TIMEOUT",
                        message: format!("Phase 3 timed out: completed {completed}/{total} topics"),
                    });
                    break;
                }
            }
        })
        .await;
}

async fn send_phase_start(ctx: &mut AttemptContext, phase: SyncPhase) -> AttemptAction {
    send_frame(
        ctx,
        &serde_json::json!({"type": "PHASE_START", "phase": phase}),
    )
    .await
}

async fn send_frame<T: Serialize>(ctx: &mut AttemptContext, frame: &T) -> AttemptAction {
    let text = match serde_json::to_string(frame) {
        Ok(text) => text,
        Err(error) => {
            return fail_attempt(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                format!("Failed to serialize sync frame: {error}"),
            )
            .await;
        }
    };
    match send_ws_with_deadline(&mut ctx.ws, Message::Text(text.into())).await {
        Ok(()) => AttemptAction::Continue,
        Err(_) => AttemptAction::Stop,
    }
}

pub(crate) async fn schedule_manifest_deadline(ctx: &AttemptContext, expected_phase: u8) {
    ctx.task_tracker
        .spawn(enforce_manifest_response_deadline(
            ctx.expected_manifest_types.clone(),
            ctx.manifest_phase.clone(),
            expected_phase,
            ctx.tx.clone(),
            ctx.attempt_id,
            PHASE_RESPONSE_TIMEOUT,
        ))
        .await;
}

fn clear_manifest_expectation(ctx: &AttemptContext) {
    ctx.expected_manifest_count.store(0, Ordering::SeqCst);
    ctx.manifest_responses_received.store(0, Ordering::SeqCst);
    if let Ok(mut expected) = ctx.expected_manifest_types.lock() {
        expected.clear();
    }
}

pub(crate) fn set_manifest_expectation_for_command(
    ctx: &AttemptContext,
    values: HashSet<ManifestType>,
) {
    set_manifest_expectation(ctx, values);
}

fn set_manifest_expectation(ctx: &AttemptContext, values: HashSet<ManifestType>) {
    ctx.expected_manifest_count
        .store(values.len() as u32, Ordering::SeqCst);
    ctx.manifest_responses_received.store(0, Ordering::SeqCst);
    if let Ok(mut expected) = ctx.expected_manifest_types.lock() {
        *expected = values;
    }
}

fn emit_sync_log(ctx: &AttemptContext, level: &str, message: &str) {
    super::logs::emit_sync_log(&ctx.app, level, message);
}

async fn fail_attempt(
    ctx: &mut AttemptContext,
    code: &'static str,
    message: String,
) -> AttemptAction {
    super::commands::fail(ctx, code, message, Vec::new()).await
}
