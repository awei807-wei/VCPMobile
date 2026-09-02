use super::attempt::{AttemptAction, AttemptContext};
use super::batching::build_diff_batches;
use super::protocol::{
    enforce_manifest_response_deadline, enforce_topic_hash_response_deadline,
    send_ws_with_deadline, PHASE3_WATCHDOG_STUCK_TICKS, PHASE3_WATCHDOG_TICK,
    PHASE_RESPONSE_TIMEOUT,
};
use super::types::SyncCommand;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_pipeline::{Phase1Metadata, Phase3Message};
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use tauri::Manager;
use tokio_tungstenite::tungstenite::protocol::Message;

pub(crate) async fn handle_pipeline_command(
    ctx: &mut AttemptContext,
    command: crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand,
) -> AttemptAction {
    match command {
        crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicMetadata => {
            start_topic_metadata(ctx).await
        }
        crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartTopicValidation => {
            start_topic_validation(ctx).await
        }
        crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::StartMessages => {
            start_messages(ctx).await
        }
        crate::vcp_modules::sync_pipeline::pipeline::PipelineCommand::Finalize => {
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
    let db = ctx.app.state::<DbState>();
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
    let manifest = match Phase1Metadata::build_targeted_topic_manifest(&db.pool, &owners).await {
        Ok(manifest) => manifest,
        Err(error) => {
            return fail_attempt(ctx, "TOPIC_MANIFEST_DB_FAILED", error.to_string()).await;
        }
    };
    if manifest.data_type != SyncDataType::Topic {
        return fail_attempt(
            ctx,
            "TOPIC_MANIFEST_INVALID",
            "Targeted topic manifest returned an unexpected data type".to_string(),
        )
        .await;
    }
    ctx.manifest_phase.store(3, Ordering::SeqCst);
    set_manifest_expectation(ctx, ["topic"]);
    send_manifest(ctx, manifest.items, manifest.data_type, 2, owners).await
}

async fn start_topic_validation(ctx: &mut AttemptContext) -> AttemptAction {
    emit_sync_log(ctx, "info", "=== Phase 2.5: Validating Topic Hashes ===");
    let (hashes, topics) = match collect_topic_hashes(ctx).await {
        Ok(value) => value,
        Err(action) => return action,
    };
    let overlap = {
        let mut expected = ctx.expected_topic_hash_results.lock().await;
        if expected.is_some() {
            true
        } else {
            *expected = Some(hashes.keys().cloned().collect());
            false
        }
    };
    if overlap {
        return fail_attempt(
            ctx,
            "TOPIC_HASH_RESPONSE_OVERLAP",
            "A topic hash response is already pending".to_string(),
        )
        .await;
    }
    let value = json!({"type": "SYNC_TOPIC_HASH_BATCH_V2", "hashes": hashes, "topics": topics});
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_err()
    {
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

async fn collect_topic_hashes(
    ctx: &mut AttemptContext,
) -> Result<
    (
        serde_json::Map<String, serde_json::Value>,
        Vec<serde_json::Value>,
    ),
    AttemptAction,
> {
    let db = ctx.app.state::<DbState>();
    let owners = ctx
        .changed_owners
        .lock()
        .await
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let topic_hashes = match Phase3Message::get_targeted_topic_hashes(&db.pool, &owners).await {
        Ok(value) => value,
        Err(error) => {
            return Err(fail_attempt(ctx, "TOPIC_HASH_DB_FAILED", error.to_string()).await);
        }
    };
    if topic_hashes.len() > super::protocol::MAX_SYNC_TOPICS {
        return Err(fail_attempt(
            ctx,
            "TOPIC_HASH_BUDGET_EXCEEDED",
            "Topic hash batch exceeds the configured topic budget".to_string(),
        )
        .await);
    }
    let mut hashes = serde_json::Map::new();
    let mut topics = Vec::new();
    for (topic_id, state) in topic_hashes {
        hashes.insert(
            topic_id.clone(),
            json!({"configHash": state.config_hash, "contentHash": state.content_hash}),
        );
        topics.push(json!({"topicId": topic_id, "ownerType": state.owner_type, "ownerId": state.owner_id, "configHash": state.config_hash, "contentHash": state.content_hash}));
    }
    Ok((hashes, topics))
}

async fn start_messages(ctx: &mut AttemptContext) -> AttemptAction {
    emit_sync_log(ctx, "info", "=== Phase 3: Messages ===");
    if send_phase_start(ctx, "messages").await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    let changed_ids = ctx.changed_topics.lock().await.clone();
    if changed_ids.is_empty() {
        let _ = ctx.tx.send(SyncCommand::Finalize {
            attempt_id: ctx.attempt_id,
        });
        return AttemptAction::Continue;
    }
    let db = ctx.app.state::<DbState>();
    let states = match Phase3Message::get_topic_message_hashes(&db.pool, &changed_ids).await {
        Ok(value) => value,
        Err(error) => {
            return fail_attempt(ctx, "PHASE3_HASH_PREP_FAILED", error.to_string()).await;
        }
    };
    ctx.pending_topics
        .total
        .store(states.len(), Ordering::SeqCst);
    clear_phase3_tracker(ctx).await;
    let batches = match build_diff_batches(states) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => {
            return fail_attempt(
                ctx,
                "PHASE3_DIFF_MISSING",
                "Phase 3 produced no request batch for changed topics".to_string(),
            )
            .await;
        }
        Err(error) => {
            return fail_attempt(ctx, "PHASE3_DIFF_BUDGET_EXCEEDED", error).await;
        }
    };
    let first = {
        let mut pending = ctx.pending_batches.lock().await;
        *pending = batches;
        pending.pop_front()
    };
    let Some(first) = first else {
        return AttemptAction::Stop;
    };
    send_batch(ctx, first).await
}

async fn send_phase_start(ctx: &mut AttemptContext, phase: &str) -> AttemptAction {
    let value = json!({"type": "PHASE_START", "phase": phase});
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_ok()
    {
        AttemptAction::Continue
    } else {
        AttemptAction::Stop
    }
}

async fn send_batch(
    ctx: &mut AttemptContext,
    batch: serde_json::Map<String, serde_json::Value>,
) -> AttemptAction {
    {
        let mut expected = ctx.expected_phase3_batch.lock().await;
        *expected = batch.keys().cloned().collect();
    }
    let value = json!({"type": "SYNC_MESSAGE_DIFF_BATCH", "topics": batch});
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_err()
    {
        return AttemptAction::Stop;
    }
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
    AttemptAction::Continue
}

async fn clear_phase3_tracker(ctx: &mut AttemptContext) {
    ctx.pending_topics.completed.lock().await.clear();
    ctx.pending_topics.modified.lock().await.clear();
    ctx.pending_topics.failed.lock().await.clear();
    ctx.pending_topics
        .legacy_attachment_warnings
        .store(0, Ordering::SeqCst);
}

fn clear_manifest_expectation(ctx: &AttemptContext) {
    ctx.expected_manifest_count.store(0, Ordering::SeqCst);
    ctx.manifest_responses_received.store(0, Ordering::SeqCst);
    if let Ok(mut expected) = ctx.expected_manifest_types.lock() {
        expected.clear();
    }
}

fn set_manifest_expectation<const N: usize>(ctx: &AttemptContext, values: [&str; N]) {
    ctx.expected_manifest_count
        .store(N as u32, Ordering::SeqCst);
    ctx.manifest_responses_received.store(0, Ordering::SeqCst);
    if let Ok(mut expected) = ctx.expected_manifest_types.lock() {
        *expected = values.into_iter().map(str::to_string).collect();
    }
}

pub(crate) fn set_manifest_expectation_for_command(ctx: &AttemptContext, values: HashSet<String>) {
    ctx.expected_manifest_count
        .store(values.len() as u32, Ordering::SeqCst);
    ctx.manifest_responses_received.store(0, Ordering::SeqCst);
    if let Ok(mut expected) = ctx.expected_manifest_types.lock() {
        *expected = values;
    }
}

async fn send_manifest(
    ctx: &mut AttemptContext,
    items: Vec<crate::vcp_modules::sync_types::EntityState>,
    data_type: SyncDataType,
    phase: u8,
    owners: Vec<String>,
) -> AttemptAction {
    let value = json!({"type": "PHASE_START", "phase": "topic_metadata"});
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_err()
    {
        return AttemptAction::Stop;
    }
    let value = json!({"type": "SYNC_MANIFEST", "data": items, "dataType": data_type, "phase": phase, "targetedOwners": owners});
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_err()
    {
        return AttemptAction::Stop;
    }
    ctx.task_tracker
        .spawn(enforce_manifest_response_deadline(
            ctx.expected_manifest_types.clone(),
            ctx.manifest_phase.clone(),
            3,
            ctx.tx.clone(),
            ctx.attempt_id,
            PHASE_RESPONSE_TIMEOUT,
        ))
        .await;
    AttemptAction::Continue
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
