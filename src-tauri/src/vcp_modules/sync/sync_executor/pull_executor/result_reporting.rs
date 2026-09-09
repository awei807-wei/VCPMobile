use super::frame_validation::TopicNDJSONFrame;
use super::{BatchPullResult, PullProgressContext};
use tauri::{AppHandle, Emitter, Runtime};

pub(crate) fn emit_warning_logs<R: Runtime>(app: &AppHandle<R>, frame: &TopicNDJSONFrame) {
    for warning in &frame.warning_samples {
        crate::vcp_modules::sync::sync_service::emit_sync_log(
            app,
            "warning",
            &format!("旧附件已省略: {warning}"),
        );
    }
}

pub(crate) fn topic_result(
    topic: crate::vcp_modules::topic_types::TopicKey,
    success: bool,
    parsed_count: usize,
    failed_count: usize,
    legacy_attachment_warnings: usize,
    error: Option<String>,
) -> BatchPullResult {
    BatchPullResult {
        topic,
        success,
        parsed_count,
        failed_count,
        legacy_attachment_warnings,
        error,
    }
}

pub(crate) fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    progress: Option<&PullProgressContext>,
    completed_topics: usize,
) {
    let Some(context) = progress else {
        return;
    };
    let _ = app.emit(
        "vcp-sync-progress",
        progress_payload(context, completed_topics),
    );
}

pub(crate) fn progress_payload(
    context: &PullProgressContext,
    completed_topics: usize,
) -> serde_json::Value {
    let completed = context.base_completed + completed_topics;
    serde_json::json!({
        "sessionId": context.session_id,
        "attemptId": context.attempt_id,
        "phase": "messages",
        "total": context.total,
        "completed": completed,
        "message": format!("Syncing Messages: {completed}/{}", context.total),
        "successfulTopics": completed,
        "totalTopics": context.total,
        "failedTopics": context.failed,
        "legacyAttachmentWarnings": context.legacy_attachment_warnings,
    })
}
