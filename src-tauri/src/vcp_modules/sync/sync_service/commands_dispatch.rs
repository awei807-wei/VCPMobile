use super::super::attempt::{AttemptAction, AttemptContext};
use super::super::types::SyncCommand;
use crate::vcp_modules::sync_error::is_attempt_restart_code;
use crate::vcp_modules::sync_types::{
    DeleteNotificationFrame, MessageDiffRequestFrame, MessageDiffTopicState,
};
use std::sync::atomic::Ordering;

pub(super) async fn handle_command(
    ctx: &mut AttemptContext,
    command: SyncCommand,
) -> AttemptAction {
    match command {
        SyncCommand::Cancel => {
            ctx.close().await;
            AttemptAction::Stop
        }
        SyncCommand::StartManualSync => super::start_manual(ctx).await,
        SyncCommand::StartAvatarMetadata { attempt_id } => {
            start_avatar_metadata(ctx, attempt_id).await
        }
        SyncCommand::StartTopicMetadata { attempt_id } => {
            start_topic_metadata(ctx, attempt_id).await
        }
        SyncCommand::StartTopicValidation { attempt_id } => {
            start_topic_validation(ctx, attempt_id).await
        }
        SyncCommand::StartMessages { attempt_id } => start_messages(ctx, attempt_id).await,
        SyncCommand::Finalize { attempt_id } => finalize(ctx, attempt_id).await,
        SyncCommand::NotifyDelete { target, deleted_at } => {
            super::send_frame(ctx, &DeleteNotificationFrame::new(target, deleted_at)).await
        }
        SyncCommand::SendMessageDiff { attempt_id, topics } => {
            send_message_diff(ctx, attempt_id, topics).await
        }
        SyncCommand::Phase3BatchFinished { attempt_id, result } => {
            phase3_batch_finished(ctx, attempt_id, result).await
        }
        SyncCommand::FailAttempt {
            attempt_id,
            code,
            message,
        } => fail_attempt(ctx, attempt_id, code, message).await,
        SyncCommand::FailAttemptDetailed {
            attempt_id,
            code,
            message,
            failed_topic_ids,
        } => fail_attempt_detailed(ctx, attempt_id, code, message, failed_topic_ids).await,
    }
}

async fn start_avatar_metadata(ctx: &mut AttemptContext, attempt_id: u64) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::start_avatar_metadata(ctx).await
    }
}

async fn start_topic_metadata(ctx: &mut AttemptContext, attempt_id: u64) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::start_topic_metadata(ctx).await
    }
}

async fn start_topic_validation(ctx: &mut AttemptContext, attempt_id: u64) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::start_topic_validation(ctx).await
    }
}

async fn start_messages(ctx: &mut AttemptContext, attempt_id: u64) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::start_messages(ctx).await
    }
}

async fn finalize(ctx: &mut AttemptContext, attempt_id: u64) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::finalize(ctx).await
    }
}

async fn send_message_diff(
    ctx: &mut AttemptContext,
    attempt_id: u64,
    topics: Vec<MessageDiffTopicState>,
) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        return AttemptAction::Continue;
    }
    let frame = MessageDiffRequestFrame::new(topics);
    match frame.validate() {
        Ok(()) => super::send_frame(ctx, &frame).await,
        Err(message) => super::fail(ctx, "PROTOCOL_FRAME_INVALID", message, Vec::new()).await,
    }
}

async fn phase3_batch_finished(
    ctx: &mut AttemptContext,
    attempt_id: u64,
    result: Result<(), crate::vcp_modules::sync_executor::batch_diff_handler::Phase3ProtocolError>,
) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        return AttemptAction::Continue;
    }
    ctx.phase3_inflight.store(false, Ordering::SeqCst);
    match result {
        Ok(()) => AttemptAction::Continue,
        Err(error) if is_attempt_restart_code(&error.code) => {
            ctx.mark_retry(&error.code, error.message);
            ctx.close().await;
            AttemptAction::Stop
        }
        Err(error) => super::fail(ctx, &error.code, error.message, error.failed_topic_ids).await,
    }
}

async fn fail_attempt(
    ctx: &mut AttemptContext,
    attempt_id: u64,
    code: &'static str,
    message: String,
) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::fail(ctx, code, message, Vec::new()).await
    }
}

async fn fail_attempt_detailed(
    ctx: &mut AttemptContext,
    attempt_id: u64,
    code: String,
    message: String,
    failed_topic_ids: Vec<String>,
) -> AttemptAction {
    if ctx.stale_attempt(attempt_id) {
        AttemptAction::Continue
    } else {
        super::fail(ctx, &code, message, failed_topic_ids).await
    }
}
