use super::attempt::{AttemptAction, AttemptContext};
use super::commands::fail;
use super::entity::{handle_entity_delete, handle_entity_update};
use super::errors::{publish_sync_completed, publish_sync_error};
use super::protocol::consume_final_ack;
use super::types::SyncCommand;
use crate::vcp_modules::sync::wire_frame::parse_inbound_frame;
use crate::vcp_modules::sync::wire_protocol::{
    parse_desktop_diagnostic_frame, DesktopDiagnosticFrame,
};
use crate::vcp_modules::sync_error::{encode_wire_sync_error, parse_wire_sync_error_frame};
use crate::vcp_modules::sync_executor::batch_diff_handler::{
    parse_phase3_batch_frame, BatchDiffHandler, Phase3Context, Phase3ContextParams,
};
use crate::vcp_modules::sync_executor::diff_handler::{
    DiffContext, DiffContextParams, DiffHandler,
};
use crate::vcp_modules::sync_types::SyncDataType;
use futures_util::SinkExt;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::protocol::Message;

pub(crate) async fn handle_ws_result(
    ctx: &mut AttemptContext,
    result: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> AttemptAction {
    match result {
        Some(Ok(Message::Text(text))) => handle_text(ctx, &text).await,
        Some(Ok(Message::Ping(payload))) => {
            if ctx.ws.send(Message::Pong(payload)).await.is_ok() {
                AttemptAction::Continue
            } else {
                AttemptAction::Stop
            }
        }
        Some(Ok(Message::Pong(_))) => AttemptAction::Continue,
        Some(Ok(Message::Close(frame))) => handle_close(ctx, frame).await,
        Some(Ok(Message::Binary(_))) => {
            fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                "Binary WebSocket frames are not valid Wire 1.2 business frames".to_string(),
                Vec::new(),
            )
            .await
        }
        Some(Ok(Message::Frame(_))) => {
            fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                "Raw WebSocket frames are not valid Wire 1.2 business frames".to_string(),
                Vec::new(),
            )
            .await
        }
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

async fn handle_text(ctx: &mut AttemptContext, text: &str) -> AttemptAction {
    let frame = match parse_inbound_frame(text) {
        Ok(frame) => frame,
        Err(error) => return fail(ctx, error.code, error.message, Vec::new()).await,
    };
    match frame.frame_type.as_str() {
        "SYNC_ENTITY_UPDATE" => handle_entity_update(ctx, &frame.payload).await,
        "SYNC_DELETE_NOTIFY" => handle_entity_delete(ctx, &frame.payload).await,
        "SYNC_ERROR" => handle_remote_error(ctx, &frame.payload).await,
        "SYNC_DIFF_RESULTS" => handle_diff_results(ctx, &frame.payload).await,
        "SYNC_DIFF_RESULTS_BATCH" => handle_diff_batch(ctx, text).await,
        "SYNC_TOPIC_HASH_RESULTS" => handle_topic_hash_results(ctx, &frame.payload).await,
        "PHASE_ACK" => handle_phase_ack(ctx, &frame.payload).await,
        "SYNC_LOG_EVENT" | "DESKTOP_PHASE_START" | "DESKTOP_PHASE_COMPLETE" => {
            handle_desktop_diagnostic(ctx, &frame.payload).await
        }
        _ => {
            fail(
                ctx,
                "PROTOCOL_FRAME_INVALID",
                format!("Unknown sync protocol frame type {}", frame.frame_type),
                Vec::new(),
            )
            .await
        }
    }
}

async fn handle_desktop_diagnostic(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let frame = match parse_desktop_diagnostic_frame(payload) {
        Ok(frame) => frame,
        Err(message) => {
            return fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await;
        }
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
    let failed_topic_ids = wire.failed_topic_ids.clone();
    let encoded = match encode_wire_sync_error(&wire) {
        Ok(value) => value,
        Err(message) => {
            return fail(ctx, "PROTOCOL_FRAME_INVALID", message, failed_topic_ids).await
        }
    };
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

async fn handle_diff_results(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let Some(data_type) = payload
        .get("dataType")
        .and_then(|value| serde_json::from_value::<SyncDataType>(value.clone()).ok())
    else {
        return fail(
            ctx,
            "PROTOCOL_FRAME_INVALID",
            "SYNC_DIFF_RESULTS.dataType is missing or invalid".to_string(),
            Vec::new(),
        )
        .await;
    };
    let context = DiffContext::new(DiffContextParams {
        app_handle: &ctx.app,
        data_type,
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
        logger: &ctx.logger,
        task_tracker: &ctx.task_tracker,
        session_id: ctx.session_id,
        attempt_id: ctx.attempt_id,
    });
    let result = DiffHandler::handle_diff(context, payload).await;
    match result {
        Ok(()) => AttemptAction::Continue,
        Err(error) => fail(ctx, "SYNC_DIFF_HANDLER_FAILED", error, Vec::new()).await,
    }
}

async fn handle_diff_batch(ctx: &mut AttemptContext, text: &str) -> AttemptAction {
    let payload = match parse_phase3_batch_frame(text) {
        Ok(value) => value,
        Err(error) => return fail(ctx, &error.code, error.message, error.failed_topic_ids).await,
    };
    if ctx.phase3_inflight.swap(true, Ordering::SeqCst) {
        return fail(
            ctx,
            "PHASE3_BATCH_OVERLAP",
            "Received a Phase 3 batch while another batch is still in flight".to_string(),
            Vec::new(),
        )
        .await;
    }
    let app = ctx.app.clone();
    let http = ctx.http.clone();
    let base = ctx.http_url.clone();
    let token = ctx.token.clone();
    let tracker = ctx.pending_topics.clone();
    let handler_tx = ctx.tx.clone();
    let batch_tx = ctx.tx.clone();
    let logger = ctx.logger.clone();
    let queue = ctx.write_queue.clone();
    let pending = ctx.pending_batches.clone();
    let uploaded = ctx.uploaded_hashes.clone();
    let expected = ctx.expected_phase3_batch.clone();
    let prerender = ctx.prerender_enabled;
    let attempt_id = ctx.attempt_id;
    let context = Phase3Context::new(Phase3ContextParams {
        app_handle: &app,
        http_client: &http,
        base_url: &base,
        token: &token,
        tracker: &tracker,
        tx_internal: &handler_tx,
        logger: &logger,
        write_queue: &queue,
        pending_diff_batches: &pending,
        prerender_enabled: prerender,
        uploaded_hashes: &uploaded,
        expected_batch_topics: &expected,
        attempt_id,
    });
    ctx.task_tracker
        .spawn(async move {
            let result = BatchDiffHandler::handle_diff_batch(context, &payload).await;
            let _ = batch_tx.send(SyncCommand::Phase3BatchFinished { attempt_id, result });
        })
        .await;
    AttemptAction::Continue
}

async fn handle_topic_hash_results(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let expected = ctx.expected_topic_hash_results.lock().await.take();
    let changed = match expected {
        Some(expected) => parse_topic_hash_result(payload, &expected),
        None => {
            Err("Received an unexpected or duplicate SYNC_TOPIC_HASH_RESULTS frame".to_string())
        }
    };
    match changed {
        Ok(changed) => {
            *ctx.changed_topics.lock().await = changed;
            let _ = ctx.tx.send(SyncCommand::StartMessages {
                attempt_id: ctx.attempt_id,
            });
            AttemptAction::Continue
        }
        Err(message) => fail(ctx, "TOPIC_HASH_RESULTS_INVALID", message, Vec::new()).await,
    }
}

pub(crate) fn parse_topic_hash_result(
    payload: &Value,
    expected: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "SYNC_TOPIC_HASH_RESULTS must be an object".to_string())?;
    if object.len() != 2
        || object.get("type").and_then(Value::as_str) != Some("SYNC_TOPIC_HASH_RESULTS")
        || !object.contains_key("changedTopics")
    {
        return Err(
            "SYNC_TOPIC_HASH_RESULTS must contain exactly type and changedTopics".to_string(),
        );
    }
    parse_topic_ids(object.get("changedTopics"), expected)
}

fn parse_topic_ids(
    value: Option<&Value>,
    expected: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let values = value
        .and_then(Value::as_array)
        .ok_or_else(|| "SYNC_TOPIC_HASH_RESULTS.changedTopics must be an array".to_string())?;
    if values.len() > super::protocol::MAX_SYNC_TOPICS {
        return Err("SYNC_TOPIC_HASH_RESULTS.changedTopics exceeds topic budget".to_string());
    }
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let id = value.as_str().filter(|id| !id.is_empty()).ok_or_else(|| {
            "SYNC_TOPIC_HASH_RESULTS.changedTopics must contain non-empty strings".to_string()
        })?;
        if !seen.insert(id.to_string()) || !expected.contains(id) {
            return Err(format!(
                "SYNC_TOPIC_HASH_RESULTS contains unexpected or duplicate topic {id}"
            ));
        }
        result.push(id.to_string());
    }
    Ok(result)
}

async fn handle_phase_ack(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    let pending = ctx
        .awaiting_final_ack
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    match pending {
        Some(expected) if consume_final_ack(&ctx.awaiting_final_ack, payload) => {
            ctx.manifest_phase.store(0, Ordering::SeqCst);
            if let Err(error) = ctx.write_queue.flush().await {
                return fail(
                    ctx,
                    "FINAL_WRITE_DRAIN_FAILED",
                    error.to_string(),
                    Vec::new(),
                )
                .await;
            }
            let summary = ctx.pending_topics.completion_summary().await;
            ctx.success =
                publish_sync_completed(&ctx.app, ctx.session_id, &ctx.status, summary).await;
            ctx.close().await;
            AttemptAction::Stop
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
        None => validate_intermediate_ack(ctx, payload).await,
    }
}

async fn validate_intermediate_ack(ctx: &mut AttemptContext, payload: &Value) -> AttemptAction {
    if is_valid_intermediate_ack(payload) {
        AttemptAction::Continue
    } else {
        fail(ctx, "FINAL_ACK_INVALID", "PHASE_ACK must contain exactly type and phase until the final acknowledgement is pending".to_string(), Vec::new()).await
    }
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
