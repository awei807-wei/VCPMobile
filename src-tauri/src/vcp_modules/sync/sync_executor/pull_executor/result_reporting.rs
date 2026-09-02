use super::frame_validation::TopicNDJSONFrame;
use super::message_persistence::process_topic_messages;
use super::{BatchPullResult, PullProgressContext};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync::sync_error::WireSyncError;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::{mpsc, OwnedSemaphorePermit};
use tokio::task::JoinSet;

pub(crate) fn emit_warning_logs<R: Runtime>(app: &AppHandle<R>, frame: &TopicNDJSONFrame) {
    for warning in &frame.warning_samples {
        crate::vcp_modules::sync::sync_service::emit_sync_log(
            app,
            "warning",
            &format!("旧附件已省略: {warning}"),
        );
    }
}

pub(crate) async fn send_error_result(
    tx: &mpsc::Sender<BatchPullResult>,
    topic_id: String,
    warning_count: usize,
    error: WireSyncError,
) -> Result<(), String> {
    let encoded = crate::vcp_modules::sync::sync_error::encode_wire_sync_error(&error)?;
    send_result(
        tx,
        BatchPullResult {
            topic_id,
            success: false,
            parsed_count: 0,
            failed_count: 0,
            legacy_attachment_warnings: warning_count,
            error: Some(encoded),
        },
    )
    .await
}

pub(crate) async fn send_empty_result(
    tx: &mpsc::Sender<BatchPullResult>,
    topic_id: String,
    warning_count: usize,
) -> Result<(), String> {
    send_result(
        tx,
        BatchPullResult {
            topic_id,
            success: true,
            parsed_count: 0,
            failed_count: 0,
            legacy_attachment_warnings: warning_count,
            error: None,
        },
    )
    .await
}

pub(crate) async fn send_result(
    tx: &mpsc::Sender<BatchPullResult>,
    result: BatchPullResult,
) -> Result<(), String> {
    tx.send(result)
        .await
        .map_err(|_| "Pull result receiver closed".to_string())
}

pub(crate) fn spawn_topic_worker<R: Runtime>(
    workers: &mut JoinSet<()>,
    permit: OwnedSemaphorePermit,
    frame: TopicNDJSONFrame,
    app: &AppHandle<R>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
    tx: mpsc::Sender<BatchPullResult>,
) {
    let app = app.clone();
    let write_queue = write_queue.clone();
    workers.spawn(async move {
        let _permit = permit;
        let topic_id = frame.topic_id;
        let warning_count = frame.legacy_attachment_warnings;
        let messages = frame
            .messages
            .into_iter()
            .map(crate::vcp_modules::chat_manager::ChatMessage::from)
            .collect();
        let result =
            process_topic_messages(&app, &topic_id, messages, &write_queue, prerender_enabled)
                .await;
        let result = match result {
            Ok((parsed_count, failed_count)) => BatchPullResult {
                topic_id,
                success: true,
                parsed_count,
                failed_count,
                legacy_attachment_warnings: warning_count,
                error: None,
            },
            Err(error) => BatchPullResult {
                topic_id,
                success: false,
                parsed_count: 0,
                failed_count: 0,
                legacy_attachment_warnings: warning_count,
                error: Some(error),
            },
        };
        let _ = tx.send(result).await;
    });
}

pub(crate) async fn finish_workers(
    mut workers: JoinSet<()>,
    tx: mpsc::Sender<BatchPullResult>,
    receiver_handle: tokio::task::JoinHandle<Vec<BatchPullResult>>,
) -> Result<Vec<BatchPullResult>, String> {
    while let Some(result) = workers.join_next().await {
        if let Err(error) = result {
            log::warn!("[PullExecutor] Batch pull worker failed: {error}");
        }
    }
    drop(tx);
    receiver_handle
        .await
        .map_err(|error| format!("Pull result receiver failed: {error}"))
}

pub(crate) async fn receive_results<R: Runtime>(
    app: AppHandle<R>,
    mut rx: mpsc::Receiver<BatchPullResult>,
    progress: Option<PullProgressContext>,
    total: usize,
) -> Vec<BatchPullResult> {
    let mut results = Vec::new();
    let mut completed = 0usize;
    let mut succeeded = 0usize;
    while let Some(result) = rx.recv().await {
        completed += 1;
        if result.success {
            succeeded += 1;
            crate::vcp_modules::sync::sync_service::emit_sync_log(
                &app,
                "info",
                &format!(
                    "[PullExecutor] Batch pull: topic {} completed ({completed}/{total})",
                    result.topic_id
                ),
            );
            emit_progress(&app, progress.as_ref(), succeeded);
        } else {
            crate::vcp_modules::sync::sync_service::emit_sync_log(
                &app,
                "error",
                &format!(
                    "[PullExecutor] Batch pull: topic {} FAILED ({completed}/{total}): {}",
                    result.topic_id,
                    result.error.as_deref().unwrap_or("unknown")
                ),
            );
        }
        results.push(result);
    }
    results
}

fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    progress: Option<&PullProgressContext>,
    succeeded: usize,
) {
    let Some(context) = progress else {
        return;
    };
    let completed = context.base_completed + succeeded;
    let _ = app.emit(
        "vcp-sync-progress",
        serde_json::json!({
            "sessionId": context.session_id,
            "phase": "messages",
            "total": context.total,
            "completed": completed,
            "message": format!("Syncing Messages: {completed}/{}", context.total),
            "successfulTopics": completed,
            "totalTopics": context.total,
            "failedTopics": context.failed,
            "legacyAttachmentWarnings": context.legacy_attachment_warnings,
        }),
    );
}
