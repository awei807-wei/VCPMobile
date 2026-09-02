use super::logs::{emit_operator_sync_log, emit_sync_log};
use super::types::{SyncCompletionSummary, SyncState};
use crate::vcp_modules::sync_error::{
    build_local_error_payload, build_wire_error_payload, decode_wire_sync_error, SyncErrorPayload,
};
use crate::vcp_modules::sync_logger::redact_sync_diagnostic;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::RwLock;

pub(crate) fn build_sync_error_payload(
    code: &str,
    failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    let canonical = service_error_canonical_code(code);
    let mut payload =
        build_local_error_payload(canonical.unwrap_or(code), failed_topic_ids, log_file);
    if canonical.is_some() {
        payload.code = code.to_string();
    }
    if code == "TOKEN_MISMATCH" {
        payload.message = "手机端与电脑端的同步令牌不一致".to_string();
        payload.guidance = "重新核对两端令牌后再试。".to_string();
    }
    payload
}

fn service_error_canonical_code(code: &str) -> Option<&'static str> {
    match code {
        "TOKEN_MISMATCH" => Some("SYNC_AUTH_FAILED"),
        "VCP_LOG_DISCONNECTED" => Some("SYNC_STREAM_FAILED"),
        "NETWORK_TIMEOUT"
        | "CONNECTION_REFUSED"
        | "NETWORK_UNREACHABLE"
        | "HTTP_HANDSHAKE_REJECTED"
        | "WS_UPGRADE_FAILED"
        | "HTTP_PROBE_ERROR" => Some("SYNC_STREAM_FAILED"),
        "PROTOCOL_FRAME_INVALID" | "FINAL_ACK_INVALID" => Some("PROTOCOL_INVALID"),
        "SYNC_CONFIG_MISSING" | "SYNC_CONFIG_INVALID" | "SYNC_SETTINGS_READ_FAILED" => {
            Some("INVALID_CONFIGURATION")
        }
        "WS_SEND_FAILED" | "WS_CLOSED" | "WS_RECEIVE_FAILED" | "WS_DISCONNECTED" => {
            Some("SYNC_STREAM_FAILED")
        }
        "HTTP_CLIENT_INIT_FAILED" | "SYNC_PIPELINE_FAILED" => Some("INTERNAL_ERROR"),
        "ENTITY_UPDATE_FAILED" | "ENTITY_DELETE_FAILED" => Some("SYNC_ENTITY_WRITE_FAILED"),
        "PHASE3_BATCH_OVERLAP" | "PHASE3_DIFF_MISSING" => Some("SYNC_PROTOCOL_INVALID"),
        "TOPIC_HASH_RESULTS_INVALID" => Some("SYNC_PROTOCOL_INVALID"),
        _ => None,
    }
}

pub(crate) fn encode_sync_command_error(code: &str, detail: &str) -> String {
    log::error!(
        "[SyncCommand] [{}] {}",
        code,
        redact_sync_diagnostic(detail)
    );
    let payload = build_sync_error_payload(code, Vec::new(), None);
    match serde_json::to_string(&payload) {
        Ok(value) => format!("SYNC_ERROR:{value}"),
        Err(_) => fallback_command_error(),
    }
}

fn fallback_command_error() -> String {
    "SYNC_ERROR:{\"code\":\"SYNC_ATTEMPT_FAILED\",\"category\":\"internal\",\"origin\":\"mobile_sync\",\"stage\":\"startup\",\"retryAction\":\"manual\",\"message\":\"同步组件未能正常完成本次任务\",\"guidance\":\"重启应用后重新同步；若仍失败，请保留最新日志。\",\"failedTopicIds\":[],\"logFile\":null}".to_string()
}

pub(crate) async fn publish_sync_nonterminal_status<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }
    publish_sync_status_inner(app_handle, session_id, status, next_status, message, None).await;
}

/// Preserve a structured desktop error. The `message` is deliberately treated
/// as a transport marker only; user-visible fields always come from the wire
/// contract and never from raw diagnostics.
pub(crate) async fn publish_sync_error<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    code: &str,
    message: &str,
    failed_topic_ids: Vec<String>,
) {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return;
    }
    let wire_error = decode_wire_sync_error(message);
    let diagnostic_code = wire_error.as_ref().map_or(code, |wire| wire.code.as_str());
    emit_sync_log(
        app_handle,
        "error",
        &format!("[{diagnostic_code}] {message}"),
    );
    let log_file = sync_state
        .current_log_path
        .read()
        .await
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let error = match wire_error {
        Some(wire) => build_wire_error_payload(&wire, failed_topic_ids, log_file),
        None => build_sync_error_payload(code, failed_topic_ids, log_file),
    };
    let user_message = error.message.clone();
    publish_sync_status_inner(
        app_handle,
        session_id,
        status,
        "error",
        &user_message,
        Some(error),
    )
    .await;
}

pub(crate) async fn publish_sync_status_inner<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    next_status: &str,
    message: &str,
    error: Option<SyncErrorPayload>,
) {
    {
        let mut guard = status.write().await;
        if guard.as_str() == next_status
            || matches!(
                guard.as_str(),
                "error" | "completed" | "completed_with_warnings"
            )
        {
            return;
        }
        *guard = next_status.to_string();
    }
    let mut payload = json!({
        "status": next_status,
        "message": message,
        "source": "Sync",
        "sessionId": session_id,
    });
    if let Some(error) = error {
        payload["error"] = json!(error);
    }
    let _ = app_handle.emit("vcp-sync-status", payload);
}

pub(crate) async fn publish_sync_completed(
    app_handle: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    summary: SyncCompletionSummary,
) -> bool {
    let sync_state = app_handle.state::<SyncState>();
    let _owner_commit = sync_state.owner_commit.lock().await;
    if sync_state.current_session_id.load(Ordering::SeqCst) != session_id {
        return false;
    }
    let terminal_status = if summary.legacy_attachment_warnings > 0 {
        "completed_with_warnings"
    } else {
        "completed"
    };
    {
        let mut guard = status.write().await;
        if matches!(
            guard.as_str(),
            "error" | "completed" | "completed_with_warnings"
        ) {
            return false;
        }
        *guard = terminal_status.to_string();
    }
    let _ = app_handle.emit(
        "vcp-sync-completed",
        json!({
            "source": "Sync",
            "sessionId": session_id,
            "status": terminal_status,
            "summary": summary,
            "agentsChanged": true,
            "groupsChanged": true,
            "topicsChanged": true,
            "messagesChanged": true,
        }),
    );
    let message = if terminal_status == "completed" {
        "同步完成"
    } else {
        "同步完成，但存在旧附件警告"
    };
    let _ = app_handle.emit(
        "vcp-sync-status",
        json!({
            "status": terminal_status,
            "message": message,
            "source": "Sync",
            "sessionId": session_id,
        }),
    );
    emit_operator_sync_log(app_handle, session_id, "info", message);
    true
}
