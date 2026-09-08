use super::attempt::{AttemptAction, AttemptContext};
use super::errors::publish_sync_error;
use super::phase::set_manifest_expectation_for_command;
use super::protocol::{enforce_final_ack_deadline, send_ws_with_deadline, FINAL_ACK_TIMEOUT};
use super::types::{FinalAckKey, SyncCommand};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_error::attempt_restart_code;
use crate::vcp_modules::sync_pipeline::Phase1Metadata;
use crate::vcp_modules::sync_types::{ManifestRequestFrame, ManifestType};
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::Ordering;
use tauri::Manager;
use tokio_tungstenite::tungstenite::protocol::Message;

#[path = "commands_dispatch.rs"]
mod dispatch;

pub(crate) async fn handle_command(
    ctx: &mut AttemptContext,
    command: SyncCommand,
) -> AttemptAction {
    dispatch::handle_command(ctx, command).await
}

async fn start_manual(ctx: &mut AttemptContext) -> AttemptAction {
    let db = ctx.app.state::<DbState>();
    let manifest = match Phase1Metadata::build_owner_manifest(&db.pool).await {
        Ok(value) => value,
        Err(error) => return fail(ctx, "OWNER_MANIFEST_DB_FAILED", error, Vec::new()).await,
    };
    if manifest.manifest_type() != ManifestType::Owner {
        return fail(
            ctx,
            "OWNER_MANIFEST_INVALID",
            "Owner manifest builder returned an unexpected manifest type".to_string(),
            Vec::new(),
        )
        .await;
    }
    *ctx.owner_config_baselines.lock().await = Phase1Metadata::owner_config_baselines(&manifest);
    ctx.manifest_phase.store(1, Ordering::SeqCst);
    set_manifest_expectation_for_command(ctx, [ManifestType::Owner].into_iter().collect());
    ctx.pending_tasks.store(0, Ordering::SeqCst);
    ctx.total_tasks.store(0, Ordering::SeqCst);
    if send_frame(ctx, &ManifestRequestFrame::new(manifest)).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    super::phase::schedule_manifest_deadline(ctx, 1).await;
    AttemptAction::Continue
}

async fn start_avatar_metadata(ctx: &mut AttemptContext) -> AttemptAction {
    if let Err(error) = ctx.write_queue.flush().await {
        return fail(
            ctx,
            "OWNER_METADATA_DRAIN_FAILED",
            format!("Agent/group metadata write drain failed: {error}"),
            Vec::new(),
        )
        .await;
    }
    let db = ctx.app.state::<DbState>();
    let manifest = match Phase1Metadata::build_avatar_manifest(&db.pool).await {
        Ok(value) => value,
        Err(error) => return fail(ctx, "AVATAR_MANIFEST_DB_FAILED", error, Vec::new()).await,
    };
    if manifest.manifest_type() != ManifestType::Avatar {
        return fail(
            ctx,
            "AVATAR_MANIFEST_INVALID",
            "Avatar manifest builder returned an unexpected manifest type".to_string(),
            Vec::new(),
        )
        .await;
    }
    ctx.manifest_phase.store(2, Ordering::SeqCst);
    set_manifest_expectation_for_command(ctx, [ManifestType::Avatar].into_iter().collect());
    ctx.pending_tasks.store(0, Ordering::SeqCst);
    ctx.total_tasks.store(0, Ordering::SeqCst);
    if send_frame(ctx, &ManifestRequestFrame::new(manifest)).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
    super::phase::schedule_manifest_deadline(ctx, 2).await;
    AttemptAction::Continue
}

async fn start_topic_metadata(ctx: &mut AttemptContext) -> AttemptAction {
    if let Err(error) = ctx.write_queue.flush().await {
        return fail(
            ctx,
            "OWNER_METADATA_DRAIN_FAILED",
            format!("Owner metadata write drain failed: {error}"),
            Vec::new(),
        )
        .await;
    }
    if send_value(
        ctx,
        json!({"type":"PHASE_COMPLETED","phase":"owner_metadata"}),
    )
    .await
        == AttemptAction::Stop
    {
        return AttemptAction::Stop;
    }
    if ctx.pipeline.on_owner_metadata_done().await.is_err() {
        return fail(
            ctx,
            "SYNC_PIPELINE_FAILED",
            "Unable to advance to topic metadata".to_string(),
            Vec::new(),
        )
        .await;
    }
    AttemptAction::Continue
}

async fn start_topic_validation(ctx: &mut AttemptContext) -> AttemptAction {
    if let Err(error) = ctx.write_queue.flush().await {
        return fail(
            ctx,
            "TOPIC_METADATA_DRAIN_FAILED",
            format!("Topic metadata write drain failed: {error}"),
            Vec::new(),
        )
        .await;
    }
    if send_value(
        ctx,
        json!({"type":"PHASE_COMPLETED","phase":"topic_metadata"}),
    )
    .await
        == AttemptAction::Stop
    {
        return AttemptAction::Stop;
    }
    if ctx.pipeline.on_topic_metadata_pull_done().await.is_err() {
        return fail(
            ctx,
            "SYNC_PIPELINE_FAILED",
            "Unable to advance to topic validation".to_string(),
            Vec::new(),
        )
        .await;
    }
    AttemptAction::Continue
}

async fn start_messages(ctx: &mut AttemptContext) -> AttemptAction {
    if let Err(error) = ctx.write_queue.flush().await {
        return fail(
            ctx,
            "TOPIC_VALIDATION_DRAIN_FAILED",
            format!("Topic validation write drain failed: {error}"),
            Vec::new(),
        )
        .await;
    }
    if ctx.pipeline.on_topic_validation_done().await.is_err() {
        return fail(
            ctx,
            "SYNC_PIPELINE_FAILED",
            "Unable to advance to messages".to_string(),
            Vec::new(),
        )
        .await;
    }
    AttemptAction::Continue
}

async fn finalize(ctx: &mut AttemptContext) -> AttemptAction {
    if ctx.message_phase_barrier.defer_finalize() {
        return AttemptAction::Continue;
    }
    let modified = ctx.pending_topics.modified.lock().await.clone();
    let db = ctx.app.state::<DbState>();
    if let Err(error) = crate::vcp_modules::sync::sync_finalize::SyncFinalizer::execute(
        &ctx.app,
        &db,
        &ctx.write_queue,
        &ctx.pipeline,
        &ctx.logger,
        modified,
    )
    .await
    {
        return fail(ctx, "SYNC_FINALIZATION_FAILED", error, Vec::new()).await;
    }
    let final_ack = FinalAckKey::new(ctx.session_id, ctx.attempt_id);
    let lock_failed = match ctx.awaiting_final_ack.lock() {
        Ok(mut pending) => {
            *pending = Some(final_ack.clone());
            false
        }
        Err(_) => true,
    };
    if lock_failed {
        return fail(
            ctx,
            "SYNC_STATE_POISONED",
            "Final acknowledgement state lock is poisoned".to_string(),
            Vec::new(),
        )
        .await;
    }
    if send_value(ctx, final_ack.message()).await == AttemptAction::Stop {
        clear_final_ack(ctx, &final_ack);
        return AttemptAction::Stop;
    }
    ctx.task_tracker
        .spawn(enforce_final_ack_deadline(
            ctx.awaiting_final_ack.clone(),
            final_ack,
            ctx.tx.clone(),
            FINAL_ACK_TIMEOUT,
        ))
        .await;
    AttemptAction::Continue
}

fn clear_final_ack(ctx: &AttemptContext, expected: &FinalAckKey) {
    if let Ok(mut pending) = ctx.awaiting_final_ack.lock() {
        if pending.as_ref() == Some(expected) {
            pending.take();
        }
    }
}

async fn send_frame<T: Serialize>(ctx: &mut AttemptContext, frame: &T) -> AttemptAction {
    match serde_json::to_value(frame) {
        Ok(value) => send_value(ctx, value).await,
        Err(error) => {
            fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                format!("Failed to serialize sync frame: {error}"),
                Vec::new(),
            )
            .await
        }
    }
}

async fn send_value(ctx: &mut AttemptContext, value: serde_json::Value) -> AttemptAction {
    if send_ws_with_deadline(&mut ctx.ws, Message::Text(value.to_string().into()))
        .await
        .is_ok()
    {
        AttemptAction::Continue
    } else {
        AttemptAction::Stop
    }
}

pub(crate) fn restart_code_for_failure(code: &str, detail: &str) -> Option<String> {
    attempt_restart_code(code, detail)
}

pub(crate) async fn fail(
    ctx: &mut AttemptContext,
    code: &str,
    message: String,
    failed_topic_ids: Vec<String>,
) -> AttemptAction {
    if let Some(restart_code) = restart_code_for_failure(code, &message) {
        ctx.mark_retry(&restart_code, message);
        ctx.close().await;
        return AttemptAction::Stop;
    }
    ctx.mark_fatal();
    super::logs::emit_sync_log(&ctx.app, "error", &message);
    publish_sync_error(
        &ctx.app,
        ctx.session_id,
        &ctx.status,
        code,
        &message,
        failed_topic_ids,
    )
    .await;
    ctx.close().await;
    AttemptAction::Stop
}

#[cfg(test)]
mod tests {
    use super::restart_code_for_failure;
    use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};

    #[test]
    fn owner_cas_drain_failure_restarts_the_attempt_without_fatal_success() {
        for owner_type in ["Agent", "Group"] {
            let detail = format!(
                "metadata write drain failed: {}",
                encode_local_sync_error(
                    "SYNC_SNAPSHOT_STALE",
                    SyncErrorStage::OwnerMetadata,
                    &format!("local {owner_type} changed"),
                    Vec::new(),
                )
            );
            assert_eq!(
                restart_code_for_failure("OWNER_METADATA_DRAIN_FAILED", &detail).as_deref(),
                Some("SYNC_SNAPSHOT_STALE")
            );
        }
    }

    #[test]
    fn normal_idempotent_drain_failure_does_not_enter_attempt_restart() {
        assert_eq!(
            restart_code_for_failure(
                "OWNER_METADATA_DRAIN_FAILED",
                "metadata write drain failed: database is locked"
            ),
            None
        );
        assert_eq!(
            restart_code_for_failure(
                "OWNER_METADATA_DRAIN_FAILED",
                "metadata write drain failed: SYNC_SNAPSHOT_STALE: local Agent changed"
            ),
            None
        );
    }
}
