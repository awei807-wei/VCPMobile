use super::attempt::{AttemptAction, AttemptContext};
use super::errors::publish_sync_error;
use super::phase::set_manifest_expectation_for_command;
use super::protocol::{enforce_final_ack_deadline, send_ws_with_deadline, FINAL_ACK_TIMEOUT};
use super::types::{FinalAckKey, SyncCommand};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_pipeline::Phase1Metadata;
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::json;
use std::sync::atomic::Ordering;
use tauri::Manager;
use tokio_tungstenite::tungstenite::protocol::Message;

pub(crate) async fn handle_command(
    ctx: &mut AttemptContext,
    command: SyncCommand,
) -> AttemptAction {
    match command {
        SyncCommand::Cancel => {
            ctx.close().await;
            AttemptAction::Stop
        }
        SyncCommand::StartManualSync => start_manual(ctx).await,
        SyncCommand::StartAvatarMetadata { attempt_id } => {
            if ctx.stale_attempt(attempt_id) { AttemptAction::Continue } else { start_avatar_metadata(ctx).await }
        }
        SyncCommand::StartTopicMetadata { attempt_id } => {
            if ctx.stale_attempt(attempt_id) { AttemptAction::Continue } else { start_topic_metadata(ctx).await }
        }
        SyncCommand::StartTopicValidation { attempt_id } => {
            if ctx.stale_attempt(attempt_id) { AttemptAction::Continue } else { start_topic_validation(ctx).await }
        }
        SyncCommand::StartMessages { attempt_id } => {
            if ctx.stale_attempt(attempt_id) { AttemptAction::Continue } else { start_messages(ctx).await }
        }
        SyncCommand::Finalize { attempt_id } => {
            if ctx.stale_attempt(attempt_id) { AttemptAction::Continue } else { finalize(ctx).await }
        }
        SyncCommand::NotifyLocalChange { id, data_type, hash, ts } => {
            match build_local_change_frame(id.clone(), data_type, hash, ts) {
                Ok(frame) => send_value(ctx, frame).await,
                Err(message) => fail(ctx, "PROTOCOL_FRAME_INVALID", message, vec![id]).await,
            }
        }
        SyncCommand::NotifyDelete { data_type, id, deleted_at } => {
            send_value(ctx, json!({"type":"SYNC_ENTITY_DELETE","id":id,"dataType":data_type,"deletedAt":deleted_at})).await
        }
        SyncCommand::NotifyMessageDelete { topic_id, message_id, deleted_at } => {
            send_value(ctx, json!({"type":"SYNC_ENTITY_DELETE","id":message_id,"topicId":topic_id,"dataType":SyncDataType::Message,"deletedAt":deleted_at})).await
        }
        SyncCommand::SendWsMessage { attempt_id, value } => {
            if ctx.stale_attempt(attempt_id) { return AttemptAction::Continue; }
            send_value(ctx, value).await
        }
        SyncCommand::Phase3BatchFinished { attempt_id, result } => {
            if ctx.stale_attempt(attempt_id) { return AttemptAction::Continue; }
            ctx.phase3_inflight.store(false, Ordering::SeqCst);
            match result {
                Ok(()) => send_pending_batch(ctx).await,
                Err(error) => fail(ctx, &error.code, error.message, error.failed_topic_ids).await,
            }
        }
        SyncCommand::FailAttempt { attempt_id, code, message } => {
            if ctx.stale_attempt(attempt_id) { return AttemptAction::Continue; }
            fail(ctx, code, message, Vec::new()).await
        }
        SyncCommand::FailAttemptDetailed { attempt_id, code, message, failed_topic_ids } => {
            if ctx.stale_attempt(attempt_id) { return AttemptAction::Continue; }
            fail(ctx, &code, message, failed_topic_ids).await
        }
        SyncCommand::RetryAttempt {
            attempt_id,
            code,
            message,
        } => {
            if ctx.stale_attempt(attempt_id) {
                return AttemptAction::Continue;
            }
            ctx.mark_retry(code, message.clone());
            super::logs::emit_sync_log(&ctx.app, "warning", &message);
            ctx.close().await;
            AttemptAction::Stop
        }
    }
}

pub(super) fn build_local_change_frame(
    id: String,
    data_type: SyncDataType,
    hash: String,
    ts: i64,
) -> Result<serde_json::Value, String> {
    if !matches!(
        data_type,
        SyncDataType::Agent | SyncDataType::Group | SyncDataType::Topic
    ) {
        return Err(format!("Wire 1.2 不支持 {data_type} 实体的实时更新通知"));
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Wire 1.2 实体更新 ID 包含非法字符".to_string());
    }
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Wire 1.2 实体更新哈希必须是小写 SHA-256".to_string());
    }
    if ts < 0 {
        return Err("Wire 1.2 实体更新时间戳不能为负数".to_string());
    }
    Ok(json!({
        "type": "SYNC_ENTITY_UPDATE",
        "id": id,
        "dataType": data_type,
        "hash": hash,
        "ts": ts,
    }))
}

async fn start_manual(ctx: &mut AttemptContext) -> AttemptAction {
    let db = ctx.app.state::<DbState>();
    let manifests = match build_owner_manifests(&db).await {
        Ok(value) => value,
        Err(error) => return fail(ctx, "OWNER_MANIFEST_DB_FAILED", error, Vec::new()).await,
    };
    let manifest_types = manifests
        .iter()
        .map(|manifest| manifest.data_type.to_string())
        .collect::<std::collections::HashSet<_>>();
    if manifest_types.len() != manifests.len() {
        return fail(
            ctx,
            "OWNER_MANIFEST_INVALID",
            "Phase 1 manifests contain duplicate data types".to_string(),
            Vec::new(),
        )
        .await;
    }
    set_manifest_expectation_for_command(ctx, manifest_types);
    for manifest in manifests {
        let value = json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type,"phase":1});
        if send_value(ctx, value).await == AttemptAction::Stop {
            return AttemptAction::Stop;
        }
    }
    AttemptAction::Continue
}

async fn build_owner_manifests(
    db: &DbState,
) -> Result<Vec<crate::vcp_modules::sync_types::SyncManifest>, String> {
    Ok(vec![
        Phase1Metadata::build_agent_manifest(&db.pool).await?,
        Phase1Metadata::build_group_manifest(&db.pool).await?,
    ])
}

async fn start_avatar_metadata(ctx: &mut AttemptContext) -> AttemptAction {
    if ctx.write_queue.flush().await.is_err() {
        return fail(
            ctx,
            "OWNER_METADATA_DRAIN_FAILED",
            "Agent/group metadata write drain failed".to_string(),
            Vec::new(),
        )
        .await;
    }
    let db = ctx.app.state::<DbState>();
    let manifest = match Phase1Metadata::build_avatar_manifest(&db.pool).await {
        Ok(value) => value,
        Err(error) => return fail(ctx, "AVATAR_MANIFEST_DB_FAILED", error, Vec::new()).await,
    };
    ctx.manifest_phase.store(2, Ordering::SeqCst);
    set_manifest_expectation_for_command(
        ctx,
        [manifest.data_type.to_string()].into_iter().collect(),
    );
    send_value(ctx, json!({"type":"SYNC_MANIFEST","data":manifest.items,"dataType":manifest.data_type,"phase":1})).await
}

async fn start_topic_metadata(ctx: &mut AttemptContext) -> AttemptAction {
    if ctx.write_queue.flush().await.is_err() {
        return fail(
            ctx,
            "OWNER_METADATA_DRAIN_FAILED",
            "Owner metadata write drain failed".to_string(),
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
    if ctx.write_queue.flush().await.is_err() {
        return fail(
            ctx,
            "TOPIC_METADATA_DRAIN_FAILED",
            "Topic metadata write drain failed".to_string(),
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
    if ctx.write_queue.flush().await.is_err() {
        return fail(
            ctx,
            "TOPIC_VALIDATION_DRAIN_FAILED",
            "Topic validation write drain failed".to_string(),
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
    if send_value(ctx, final_ack.message()).await == AttemptAction::Stop {
        return AttemptAction::Stop;
    }
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

async fn send_pending_batch(ctx: &mut AttemptContext) -> AttemptAction {
    let next = ctx.pending_batches.lock().await.pop_front();
    let Some(batch) = next else {
        return AttemptAction::Continue;
    };
    {
        let mut expected = ctx.expected_phase3_batch.lock().await;
        *expected = batch.keys().cloned().collect();
    }
    send_value(
        ctx,
        json!({"type":"SYNC_MESSAGE_DIFF_BATCH","topics":batch}),
    )
    .await
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

pub(crate) async fn fail(
    ctx: &mut AttemptContext,
    code: &str,
    message: String,
    failed_topic_ids: Vec<String>,
) -> AttemptAction {
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
