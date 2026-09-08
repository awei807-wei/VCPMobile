use super::attempt::{AttemptAction, AttemptContext};
use super::commands::fail;
use super::errors::{publish_sync_completed, publish_sync_error};
use super::protocol::consume_final_ack;
use super::types::SyncCommand;
use crate::vcp_modules::sync::wire_protocol::{
    parse_desktop_diagnostic_frame, DesktopDiagnosticFrame,
};
use crate::vcp_modules::sync_error::{
    encode_wire_sync_error, is_attempt_restart_code, parse_wire_sync_error_frame,
};
use crate::vcp_modules::sync_executor::batch_diff_handler::BatchDiffHandler;
use crate::vcp_modules::sync_executor::diff_handler::{
    DiffContext, DiffContextParams, DiffHandler,
};
use crate::vcp_modules::sync_types::{
    ManifestResultFrame, MessageDiffResultFrame, TopicDiffResultFrame,
};
use crate::vcp_modules::topic_types::TopicKey;
use futures_util::SinkExt;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::protocol::Message;

#[path = "frame_parsing.rs"]
mod frame_parsing;
use frame_parsing::{parse_business_frame, parse_typed};

pub(crate) async fn handle_ws_result(
    ctx: &mut AttemptContext,
    result: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> AttemptAction {
    match result {
        Some(Ok(Message::Text(text))) => handle_text(ctx, &text).await,
        Some(Ok(Message::Ping(payload))) => respond_to_ping(ctx, payload).await,
        Some(Ok(Message::Pong(_))) => AttemptAction::Continue,
        Some(Ok(Message::Close(frame))) => handle_close(ctx, frame).await,
        Some(Ok(Message::Binary(_))) => invalid_non_text(ctx, "Binary").await,
        Some(Ok(Message::Frame(_))) => invalid_non_text(ctx, "Raw").await,
        Some(Err(error)) => {
            super::logs::emit_sync_log(
                &ctx.app,
                "warning",
                &format!("WebSocket receive failed: {error}"),
            );
            AttemptAction::Stop
        }
        None => AttemptAction::Stop,
    }
}

async fn respond_to_ping(
    ctx: &mut AttemptContext,
    payload: tokio_tungstenite::tungstenite::Bytes,
) -> AttemptAction {
    if ctx.ws.send(Message::Pong(payload)).await.is_ok() {
        AttemptAction::Continue
    } else {
        AttemptAction::Stop
    }
}

async fn invalid_non_text(ctx: &mut AttemptContext, kind: &str) -> AttemptAction {
    fail(
        ctx,
        "PROTOCOL_FRAME_INVALID",
        format!("{kind} WebSocket frames are not valid Wire 1.4 business frames"),
        Vec::new(),
    )
    .await
}

async fn handle_text(ctx: &mut AttemptContext, text: &str) -> AttemptAction {
    let payload = match parse_business_frame(text) {
        Ok(value) => value,
        Err((code, message)) => return fail(ctx, code, message, Vec::new()).await,
    };
    let frame_type = payload
        .get("type")
        .and_then(Value::as_str)
        .expect("parse_business_frame guarantees a type");
    match frame_type {
        "SYNC_MANIFEST_RESULT" => handle_manifest_result(ctx, payload).await,
        "SYNC_TOPIC_DIFF_RESULT" => handle_topic_diff_result(ctx, payload).await,
        "SYNC_MESSAGE_DIFF_RESULT" => handle_message_diff_result(ctx, payload).await,
        "SYNC_ERROR" => handle_remote_error(ctx, &payload).await,
        "PHASE_ACK" => handle_phase_ack(ctx, &payload).await,
        "SYNC_LOG_EVENT" | "DESKTOP_PHASE_START" | "DESKTOP_PHASE_COMPLETE" => {
            handle_desktop_diagnostic(ctx, &payload).await
        }
        other => {
            fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                format!("Unknown Wire 1.4 frame type {other}"),
                Vec::new(),
            )
            .await
        }
    }
}

async fn handle_manifest_result(ctx: &mut AttemptContext, payload: Value) -> AttemptAction {
    let frame = match parse_typed::<ManifestResultFrame>(payload, "SYNC_MANIFEST_RESULT") {
        Ok(frame) => frame,
        Err(message) => return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    };
    let manifest_type = frame.manifest_type();
    let context = DiffContext::new(DiffContextParams {
        app_handle: &ctx.app,
        manifest_type,
        http_client: &ctx.http,
        base_url: &ctx.http_url,
        token: &ctx.token,
        write_queue: &ctx.write_queue,
        pending_tasks: &ctx.pending_tasks,
        total_tasks: &ctx.total_tasks,
        manifest_responses_received: &ctx.manifest_responses_received,
        expected_manifest_count: &ctx.expected_manifest_count,
        expected_manifest_types: &ctx.expected_manifest_types,
        manifest_phase: &ctx.manifest_phase,
        tx_internal: &ctx.tx,
        changed_owners: &ctx.changed_owners,
        owner_config_baselines: &ctx.owner_config_baselines,
        logger: &ctx.logger,
        task_tracker: &ctx.task_tracker,
        session_id: ctx.session_id,
        attempt_id: ctx.attempt_id,
    });
    match DiffHandler::handle_diff(context, frame).await {
        Ok(()) => AttemptAction::Continue,
        Err(error) => fail(ctx, "SYNC_DIFF_HANDLER_FAILED", error, Vec::new()).await,
    }
}

async fn handle_topic_diff_result(ctx: &mut AttemptContext, payload: Value) -> AttemptAction {
    let frame = match parse_typed::<TopicDiffResultFrame>(payload, "SYNC_TOPIC_DIFF_RESULT") {
        Ok(frame) => frame,
        Err(message) => return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    };
    let expected = ctx.expected_topic_hash_results.lock().await.take();
    let changed = match expected {
        Some(expected) => validate_changed_topics(frame.changed_topics, &expected),
        None => Err("Received an unexpected or duplicate SYNC_TOPIC_DIFF_RESULT".to_string()),
    };
    match changed {
        Ok(changed) => {
            *ctx.changed_topics.lock().await = changed;
            let _ = ctx.tx.send(SyncCommand::StartMessages {
                attempt_id: ctx.attempt_id,
            });
            AttemptAction::Continue
        }
        Err(message) => fail(ctx, "TOPIC_DIFF_RESULT_INVALID", message, Vec::new()).await,
    }
}

pub(super) fn validate_changed_topics(
    changed: Vec<TopicKey>,
    expected: &HashSet<TopicKey>,
) -> Result<Vec<TopicKey>, String> {
    if changed.iter().any(|topic| !expected.contains(topic)) {
        return Err("SYNC_TOPIC_DIFF_RESULT contains an unexpected topic identity".to_string());
    }
    Ok(changed)
}

async fn handle_message_diff_result(ctx: &mut AttemptContext, payload: Value) -> AttemptAction {
    let frame = match parse_typed::<MessageDiffResultFrame>(payload, "SYNC_MESSAGE_DIFF_RESULT") {
        Ok(frame) => frame,
        Err(message) => return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    };
    if ctx.phase3_inflight.swap(true, Ordering::SeqCst) {
        return fail(
            ctx,
            "PHASE3_BATCH_OVERLAP",
            "Received a message diff result while another batch is in flight".to_string(),
            Vec::new(),
        )
        .await;
    }
    spawn_message_diff_handler(ctx, frame).await;
    AttemptAction::Continue
}

async fn spawn_message_diff_handler(ctx: &AttemptContext, frame: MessageDiffResultFrame) {
    let app = ctx.app.clone();
    let http = ctx.http.clone();
    let base = ctx.http_url.clone();
    let token = ctx.token.clone();
    let tracker = ctx.pending_topics.clone();
    let handler_tx = ctx.tx.clone();
    let result_tx = ctx.tx.clone();
    let logger = ctx.logger.clone();
    let queue = ctx.write_queue.clone();
    let pending = ctx.pending_batches.clone();
    let expected = ctx.expected_phase3_batch.clone();
    let expected_states = ctx.expected_phase3_states.clone();
    let prerender = ctx.prerender_enabled;
    let attempt_id = ctx.attempt_id;
    ctx.task_tracker
        .spawn(async move {
            let result = BatchDiffHandler::handle_diff_batch(
                &app,
                frame,
                &http,
                &base,
                &token,
                &tracker,
                &handler_tx,
                &logger,
                &queue,
                &pending,
                prerender,
                &expected,
                &expected_states,
                attempt_id,
            )
            .await;
            let _ = result_tx.send(SyncCommand::Phase3BatchFinished { attempt_id, result });
        })
        .await;
}

async fn handle_desktop_diagnostic(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let frame = match parse_desktop_diagnostic_frame(payload) {
        Ok(frame) => frame,
        Err(message) => return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    };
    match frame {
        DesktopDiagnosticFrame::SyncLog {
            level,
            phase,
            message,
        } => super::logs::emit_sync_log(&ctx.app, &level, &format!("[Desktop][{phase}] {message}")),
        DesktopDiagnosticFrame::PhaseStart { phase } => super::logs::emit_sync_log(
            &ctx.app,
            "info",
            &format!("[Desktop] Phase {phase} started"),
        ),
        DesktopDiagnosticFrame::PhaseComplete { phase } => super::logs::emit_sync_log(
            &ctx.app,
            "info",
            &format!("[Desktop] Phase {phase} completed"),
        ),
    }
    AttemptAction::Continue
}

async fn handle_remote_error(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let wire = match parse_wire_sync_error_frame(payload) {
        Ok(value) => value,
        Err(message) => return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    };
    let encoded = match encode_wire_sync_error(&wire) {
        Ok(value) => value,
        Err(message) => {
            return fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                message,
                wire.failed_topic_ids,
            )
            .await;
        }
    };
    if is_attempt_restart_code(&wire.code) {
        super::logs::emit_sync_log(&ctx.app, "warning", &encoded);
        ctx.mark_retry(&wire.code, encoded);
        ctx.close().await;
        return AttemptAction::Stop;
    }
    publish_sync_error(
        &ctx.app,
        ctx.session_id,
        &ctx.status,
        "REMOTE_SYNC_FAILED",
        &encoded,
        wire.failed_topic_ids,
    )
    .await;
    ctx.mark_fatal();
    ctx.close().await;
    AttemptAction::Stop
}

async fn handle_phase_ack(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let pending = ctx
        .awaiting_final_ack
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    match pending {
        Some(_) if consume_final_ack(&ctx.awaiting_final_ack, payload) => {
            complete_after_final_ack(ctx).await
        }
        Some(_) => {
            fail(
                ctx,
                "FINAL_ACK_INVALID",
                "Final PHASE_ACK did not exactly match the pending session identity".to_string(),
                Vec::new(),
            )
            .await
        }
        None if is_valid_intermediate_ack(payload) => handle_intermediate_ack(ctx, payload),
        None => {
            fail(
                ctx,
                "FINAL_ACK_INVALID",
                "Intermediate PHASE_ACK must contain exactly type and phase".to_string(),
                Vec::new(),
            )
            .await
        }
    }
}

fn handle_intermediate_ack(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    if payload.get("phase").and_then(Value::as_str) == Some("messages")
        && ctx.message_phase_barrier.acknowledge()
    {
        let _ = ctx.tx.send(SyncCommand::Finalize {
            attempt_id: ctx.attempt_id,
        });
    }
    AttemptAction::Continue
}

async fn complete_after_final_ack(ctx: &mut AttemptContext) -> AttemptAction {
    ctx.manifest_phase.store(0, Ordering::SeqCst);
    if let Err(error) = ctx.write_queue.flush().await {
        return fail(
            ctx,
            "FINAL_WRITE_DRAIN_FAILED",
            format!("Final write drain failed: {error}"),
            Vec::new(),
        )
        .await;
    }
    let summary = ctx.pending_topics.completion_summary().await;
    ctx.success = publish_sync_completed(&ctx.app, ctx.session_id, &ctx.status, summary).await;
    ctx.close().await;
    AttemptAction::Stop
}

pub(crate) fn is_valid_intermediate_ack(payload: &Value) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };
    object.len() == 2
        && object.get("type").and_then(Value::as_str) == Some("PHASE_ACK")
        && matches!(
            object.get("phase").and_then(Value::as_str),
            Some("owner_metadata" | "topic_metadata" | "messages")
        )
}

async fn handle_close(
    ctx: &mut AttemptContext,
    frame: Option<tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> AttemptAction {
    if frame
        .as_ref()
        .is_some_and(|frame| u16::from(frame.code) == 4001)
    {
        return fail(
            ctx,
            "TOKEN_MISMATCH",
            "身份认证失败（Token 错误）".to_string(),
            Vec::new(),
        )
        .await;
    }
    AttemptAction::Stop
}
