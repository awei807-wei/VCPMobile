use super::context::DiffContext;
use super::manifest::ManifestDecision;
use crate::vcp_modules::sync_service::{emit_sync_log, SyncCommand};
use crate::vcp_modules::sync_types::{ManifestAction, ManifestType};
use std::sync::atomic::Ordering;

pub(crate) async fn record_manifest(
    ctx: &DiffContext,
    manifest_type: ManifestType,
    items: &[ManifestDecision],
    current_phase: u8,
    all_manifest_types_received: bool,
) -> Result<(), String> {
    let total_ops = count_operations(items);
    log_manifest(ctx, manifest_type, items, total_ops);
    ctx.pending_tasks.fetch_add(total_ops, Ordering::SeqCst);
    ctx.total_tasks.fetch_add(total_ops, Ordering::SeqCst);
    let received = ctx
        .manifest_responses_received
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let expected = ctx.expected_manifest_count.load(Ordering::SeqCst);
    if received > expected {
        return Err(format!(
            "SYNC_MANIFEST_RESULT response count exceeds phase {current_phase} expectation"
        ));
    }
    if all_manifest_types_received && received == expected {
        advance_or_watch(ctx, current_phase).await;
    }
    Ok(())
}

fn count_operations(items: &[ManifestDecision]) -> u32 {
    items
        .iter()
        .filter(|item| item.action() != ManifestAction::Skip)
        .count() as u32
}

fn log_manifest(
    ctx: &DiffContext,
    manifest_type: ManifestType,
    items: &[ManifestDecision],
    total_ops: u32,
) {
    if total_ops == 0 {
        return;
    }
    let counts = [
        ManifestAction::Pull,
        ManifestAction::Push,
        ManifestAction::PullDelete,
        ManifestAction::PushDelete,
    ]
    .map(|action| count_action(items, action));
    let phase_tag = match manifest_type {
        ManifestType::Owner | ManifestType::Avatar => "owner_metadata",
        ManifestType::Topic => "topic_metadata",
    };
    let message = format!(
        "[{}] Diff: pull={} push={} pull_delete={} push_delete={}",
        manifest_type, counts[0], counts[1], counts[2], counts[3]
    );
    log::info!("[Sync] [{}] {}", phase_tag, message);
    emit_sync_log(&ctx.app_handle, "info", &message);
    if let Ok(mut logger) = ctx.logger.lock() {
        logger.log_operation(
            phase_tag,
            &manifest_type.to_string(),
            "manifest",
            true,
            Some(&format!(
                "pull={} push={} pull_delete={} push_delete={}",
                counts[0], counts[1], counts[2], counts[3]
            )),
        );
    }
}

fn count_action(items: &[ManifestDecision], action: ManifestAction) -> u32 {
    items.iter().filter(|item| item.action() == action).count() as u32
}

async fn advance_or_watch(ctx: &DiffContext, current_phase: u8) {
    let pending = ctx.pending_tasks.load(Ordering::SeqCst);
    log::info!(
        "[SyncService] All manifests received for Phase {}: pending={}",
        current_phase,
        pending
    );
    if pending == 0 {
        send_next_phase(ctx, current_phase);
    } else {
        spawn_watchdog(ctx, current_phase).await;
    }
}

pub(crate) fn maybe_advance(ctx: &DiffContext) {
    if ctx.pending_tasks.load(Ordering::SeqCst) != 0
        || ctx.manifest_responses_received.load(Ordering::SeqCst)
            != ctx.expected_manifest_count.load(Ordering::SeqCst)
    {
        return;
    }
    let phase = ctx.manifest_phase.load(Ordering::SeqCst);
    send_next_phase(ctx, phase);
}

fn send_next_phase(ctx: &DiffContext, phase: u8) {
    if let Some(command) = super::manifest::next_manifest_command(phase, ctx.attempt_id) {
        let _ = ctx.tx_internal.send(command);
    }
}

async fn spawn_watchdog(ctx: &DiffContext, current_phase: u8) {
    let tx = ctx.tx_internal.clone();
    let phase = ctx.manifest_phase.clone();
    let pending = ctx.pending_tasks.clone();
    let app = ctx.app_handle.clone();
    let attempt_id = ctx.attempt_id;
    ctx.task_tracker
        .spawn(async move {
            let mut last_pending = pending.load(Ordering::SeqCst);
            let mut stuck_count = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                if phase.load(Ordering::SeqCst) != current_phase {
                    break;
                }
                let current_pending = pending.load(Ordering::SeqCst);
                if current_pending == 0 {
                    break;
                }
                if current_pending == last_pending {
                    stuck_count += 1;
                    log::warn!(
                        "[SyncService] WATCHDOG: Phase {} pending count stuck at {} ({} ticks)",
                        current_phase,
                        current_pending,
                        stuck_count
                    );
                } else {
                    stuck_count = 0;
                    last_pending = current_pending;
                }
                if stuck_count >= 6 {
                    fail_stalled_phase(&app, &tx, attempt_id, current_phase, current_pending);
                    break;
                }
                if stuck_count >= 1 {
                    emit_sync_log(
                        &app,
                        "warn",
                        &format!(
                            "同步进度缓慢 (Phase {})，剩余任务: {}...",
                            current_phase, current_pending
                        ),
                    );
                }
            }
        })
        .await;
}

fn fail_stalled_phase(
    app: &tauri::AppHandle,
    tx: &tokio::sync::mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    phase: u8,
    pending: u32,
) {
    log::error!(
        "[SyncService] WATCHDOG FATAL: Phase {} DEADLOCK detected. Failing current attempt.",
        phase
    );
    emit_sync_log(
        app,
        "error",
        &format!(
            "同步流程停滞超过 60 秒 (Phase {})；当前 attempt 已失败，不会把未完成数据报告为成功。",
            phase
        ),
    );
    let _ = tx.send(SyncCommand::FailAttempt {
        attempt_id,
        code: "SYNC_PHASE_STALLED",
        message: format!("Sync phase {phase} timed out with {pending} unfinished operations"),
    });
}

pub(crate) fn complete_operation(ctx: &DiffContext) {
    complete_operations(ctx, 1);
}

pub(crate) fn complete_operations(ctx: &DiffContext, count: u32) {
    ctx.pending_tasks.fetch_sub(count, Ordering::SeqCst);
    let current_pending = ctx.pending_tasks.load(Ordering::SeqCst);
    let total = ctx.total_tasks.load(Ordering::SeqCst);
    let done = total.saturating_sub(current_pending);
    let phase = if ctx.manifest_phase.load(Ordering::SeqCst) <= 2 {
        "owner_metadata"
    } else {
        "topic_metadata"
    };
    let _ = tauri::Emitter::emit(
        &ctx.app_handle,
        "vcp-sync-progress",
        serde_json::json!({
            "sessionId": ctx.session_id,
            "attemptId": ctx.attempt_id,
            "phase": phase,
            "total": total,
            "completed": done,
            "message": format!("Syncing: {done}/{total}")
        }),
    );
    if current_pending == 0 {
        maybe_advance(ctx);
    }
}
