use super::errors::{publish_sync_error, publish_sync_nonterminal_status};
use super::logs::{emit_operator_sync_log, emit_sync_log};
use super::types::{FinalAckKey, PendingFinalAck, SyncCommand, SyncWebSocket};
use crate::vcp_modules::sync_error::encode_wire_sync_error;
use crate::vcp_modules::sync_types::ManifestType;
use crate::vcp_modules::topic_types::TopicKey;
use crate::vcp_modules::wire_protocol::handshake::{
    EXPECTED_PLUGIN_VERSION, WIRE_PROTOCOL_VERSION,
};
use crate::vcp_modules::wire_protocol::{
    parse_version_handshake_json, VersionAck, VersionHandshakeFrame,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_util::sync::CancellationToken;

pub(crate) const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const WS_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const PHASE3_WATCHDOG_TICK: Duration = Duration::from_secs(10);
pub(crate) const PHASE3_WATCHDOG_STUCK_TICKS: u32 = 6;
pub(crate) const PHASE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const FINAL_ACK_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const ENTITY_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_SYNC_RETRIES: u32 = 3;
pub(crate) const MAX_SYNC_TOPICS: usize = 10_000;

#[derive(Debug)]
pub(crate) struct RetryBudget {
    attempts: u32,
    next_delay: Duration,
}

impl RetryBudget {
    pub(crate) fn new() -> Self {
        Self {
            attempts: 0,
            next_delay: Duration::from_millis(500),
        }
    }

    pub(crate) fn attempts(&self) -> u32 {
        self.attempts
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VersionHandshakeError {
    Protocol(String),
    Remote(String),
    Closed { code: Option<u16>, reason: String },
    Transport(String),
}

/// Parse only the frames permitted while the client is waiting for VERSION_ACK.
/// Linux broadcasts one strictly shaped connection log before it receives VERSION_CHECK.
pub(crate) fn parse_version_handshake_payload(
    payload: &str,
) -> Result<Option<VersionAck>, VersionHandshakeError> {
    match parse_version_handshake_json(payload).map_err(VersionHandshakeError::Protocol)? {
        VersionHandshakeFrame::VersionAck(ack) => Ok(Some(ack)),
        VersionHandshakeFrame::SyncError(wire) => {
            let encoded = encode_wire_sync_error(&wire).map_err(VersionHandshakeError::Protocol)?;
            Err(VersionHandshakeError::Remote(encoded))
        }
        VersionHandshakeFrame::SyncLogEvent => Ok(None),
    }
}

pub(crate) async fn send_ws_with_deadline(
    ws_stream: &mut SyncWebSocket,
    message: Message,
) -> Result<(), String> {
    tokio::time::timeout(WS_OPERATION_TIMEOUT, ws_stream.send(message))
        .await
        .map_err(|_| "WebSocket send timed out".to_string())?
        .map_err(|error| error.to_string())
}

pub(crate) async fn close_ws_with_deadline(ws_stream: &mut SyncWebSocket) -> Result<(), String> {
    tokio::time::timeout(WS_OPERATION_TIMEOUT, ws_stream.close(None))
        .await
        .map_err(|_| "WebSocket close timed out".to_string())?
        .map_err(|error| error.to_string())
}

pub(crate) fn protocol_send_failure_message(context: &str, error: &str) -> String {
    format!("Failed to send {context}: {error}")
}

pub(crate) async fn terminate_after_protocol_send_failure<R: Runtime>(
    app_handle: &AppHandle<R>,
    ws_stream: &mut SyncWebSocket,
    context: &str,
    error: &str,
) {
    let message = protocol_send_failure_message(context, error);
    log::warn!("[SyncService] {message}");
    emit_sync_log(app_handle, "warning", &message);
    let _ = close_ws_with_deadline(ws_stream).await;
}

pub(crate) fn take_retry_slot(budget: &mut RetryBudget) -> Option<Duration> {
    if budget.attempts >= MAX_SYNC_RETRIES {
        return None;
    }
    budget.attempts += 1;
    let backoff = budget.next_delay;
    budget.next_delay = (budget.next_delay * 2).min(Duration::from_secs(5));
    Some(backoff)
}

pub(crate) async fn schedule_sync_retry<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    status: &Arc<tokio::sync::RwLock<String>>,
    cancel_token: &CancellationToken,
    budget: &mut RetryBudget,
    error_code: &str,
    message: &str,
) -> bool {
    let Some(backoff) = take_retry_slot(budget) else {
        let final_message =
            format!("{message}; retry budget exhausted after {MAX_SYNC_RETRIES} attempts");
        emit_sync_log(app_handle, "error", &final_message);
        publish_sync_error(
            app_handle,
            session_id,
            status,
            error_code,
            &final_message,
            Vec::new(),
        )
        .await;
        return false;
    };
    emit_sync_log(
        app_handle,
        "warning",
        &format!(
            "{message}; reconnecting after {backoff:?} ({}/{MAX_SYNC_RETRIES})",
            budget.attempts()
        ),
    );
    emit_operator_sync_log(
        app_handle,
        session_id,
        "warning",
        &format!(
            "连接中断，正在进行第 {}/{} 次自动重试",
            budget.attempts(),
            MAX_SYNC_RETRIES
        ),
    );
    publish_sync_nonterminal_status(
        app_handle,
        session_id,
        status,
        "retrying",
        &format!(
            "连接中断，{} 毫秒后进行第 {}/{} 次自动重试",
            backoff.as_millis(),
            budget.attempts(),
            MAX_SYNC_RETRIES
        ),
    )
    .await;
    !cancel_token.is_cancelled() && !cancelled_during(cancel_token, backoff).await
}

pub(crate) fn consume_final_ack(pending: &PendingFinalAck, payload: &Value) -> bool {
    let Ok(mut guard) = pending.lock() else {
        return false;
    };
    if guard
        .as_ref()
        .is_some_and(|expected| expected.matches_payload(payload))
    {
        guard.take();
        true
    } else {
        false
    }
}

pub(crate) async fn enforce_final_ack_deadline(
    pending: PendingFinalAck,
    expected: FinalAckKey,
    tx: mpsc::UnboundedSender<SyncCommand>,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    let expired = pending
        .lock()
        .map(|mut guard| {
            if guard.as_ref() == Some(&expected) {
                guard.take();
                true
            } else {
                false
            }
        })
        .unwrap_or(false);
    if expired {
        let _ = tx.send(SyncCommand::FailAttempt {
            attempt_id: expected.attempt_id,
            code: "FINAL_ACK_TIMEOUT",
            message: format!(
                "Desktop final sync acknowledgement timed out after {} seconds",
                deadline.as_secs()
            ),
        });
    }
}

pub(crate) async fn enforce_manifest_response_deadline(
    expected_types: Arc<Mutex<HashSet<ManifestType>>>,
    manifest_phase: Arc<AtomicU8>,
    expected_phase: u8,
    tx: mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    if manifest_phase.load(Ordering::SeqCst) != expected_phase {
        return;
    }
    let missing = match expected_types.lock() {
        Ok(expected) if expected.is_empty() => return,
        Ok(expected) => {
            let mut values = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
            values.sort();
            values
        }
        Err(_) => {
            let _ = tx.send(SyncCommand::FailAttemptDetailed {
                attempt_id,
                code: "SYNC_STATE_POISONED".to_string(),
                message: "Expected manifest type state is poisoned".to_string(),
                failed_topic_ids: Vec::new(),
            });
            return;
        }
    };
    let _ = tx.send(SyncCommand::FailAttemptDetailed {
        attempt_id,
        code: "MANIFEST_RESPONSE_TIMEOUT".to_string(),
        message: format!("Desktop manifest response timed out with missing data types {missing:?}"),
        failed_topic_ids: Vec::new(),
    });
}

pub(crate) async fn enforce_topic_hash_response_deadline(
    expected_results: Arc<AsyncMutex<Option<HashSet<TopicKey>>>>,
    manifest_phase: Arc<AtomicU8>,
    expected_phase: u8,
    tx: mpsc::UnboundedSender<SyncCommand>,
    attempt_id: u64,
    deadline: Duration,
) {
    tokio::time::sleep(deadline).await;
    if manifest_phase.load(Ordering::SeqCst) != expected_phase {
        return;
    }
    if let Some(pending_count) = expected_results.lock().await.as_ref().map(HashSet::len) {
        let _ = tx.send(SyncCommand::FailAttemptDetailed {
            attempt_id,
            code: "TOPIC_HASH_RESPONSE_TIMEOUT".to_string(),
            message: format!(
                "Desktop topic hash response timed out for {pending_count} expected topics"
            ),
            failed_topic_ids: Vec::new(),
        });
    }
}

pub(crate) fn parse_unique_nonempty_strings(
    value: &Value,
    field: &str,
    max_items: usize,
) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?;
    if values.len() > max_items {
        return Err(format!("{field} exceeds {max_items} item budget"));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let item = value
            .as_str()
            .filter(|item| !item.is_empty())
            .ok_or_else(|| format!("{field} must contain only non-empty strings"))?;
        if !seen.insert(item) {
            return Err(format!("{field} contains duplicate value {item}"));
        }
        result.push(item.to_string());
    }
    Ok(result)
}

pub(crate) async fn cancelled_during(token: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        biased;
        _ = token.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

pub(crate) fn expected_plugin_version() -> &'static str {
    EXPECTED_PLUGIN_VERSION
}

pub(crate) fn expected_protocol_version() -> &'static str {
    WIRE_PROTOCOL_VERSION
}

// Keep StreamExt in this module's import set: the handshake implementation in
// session.rs uses the same WebSocket stream and this documents that it is a
// stream protocol boundary, not an HTTP JSON endpoint.
pub(crate) fn _stream_marker<S: StreamExt>(_stream: &S) {}
