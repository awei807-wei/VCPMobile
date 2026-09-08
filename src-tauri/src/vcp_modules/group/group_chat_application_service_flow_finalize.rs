use super::SpeakerResult;
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat::message_service;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::vcp_client::{
    mark_message_as_error_guarded_with_channel, GuardedTransition, StreamEvent, VcpRequestError,
    VcpRequestOutcome,
};
use tauri::{ipc::Channel, AppHandle};

#[allow(clippy::too_many_arguments)]
pub(super) async fn finalize_speaker_result(
    result: Result<VcpRequestOutcome, VcpRequestError>,
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    group_id: &str,
    topic_id: &str,
    speaker: &AgentConfig,
    message_id: String,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<SpeakerResult, String> {
    match result {
        Ok(outcome) => {
            finalize_success(
                outcome,
                app,
                pool,
                group_id,
                topic_id,
                speaker,
                message_id,
                stream_channel,
            )
            .await
        }
        Err(error) => finalize_failure(error, app, pool, stream_channel).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn finalize_success(
    outcome: VcpRequestOutcome,
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    group_id: &str,
    topic_id: &str,
    speaker: &AgentConfig,
    message_id: String,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<SpeakerResult, String> {
    let VcpRequestOutcome {
        response,
        is_aborted,
        completion_lease,
        request_guard,
    } = outcome;
    let _request_guard = request_guard;
    let Some(content) = response["fullContent"].as_str() else {
        return finalize_missing_success(app, pool, &completion_lease, stream_channel).await;
    };
    let finish_reason = if is_aborted {
        Some("cancelled_by_user".to_string())
    } else {
        response["finishReason"].as_str().map(str::to_string)
    };
    let status = message_service::finalize_stream_message_guarded(
        app.clone(),
        pool,
        &completion_lease,
        content.to_string(),
        is_aborted,
        finish_reason.clone(),
        stream_channel,
        Some(speaker.id.clone()),
    )
    .await?;
    if matches!(status, message_service::StreamFinalizationStatus::Skipped) {
        return Ok(SpeakerResult::Skipped);
    }
    Ok(completed_speaker_result(
        group_id,
        topic_id,
        speaker,
        message_id,
        content,
        finish_reason,
    ))
}

async fn finalize_missing_success(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    completion_lease: &crate::vcp_modules::vcp_client::CompletionLease,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<SpeakerResult, String> {
    let mark = mark_message_as_error_guarded_with_channel(
        app,
        pool,
        completion_lease,
        Some("响应缺少 fullContent".to_string()),
        stream_channel,
    )
    .await?;
    Ok(match mark {
        GuardedTransition::Applied(()) => SpeakerResult::Failed,
        GuardedTransition::Skipped => SpeakerResult::Skipped,
    })
}

fn completed_speaker_result(
    group_id: &str,
    topic_id: &str,
    speaker: &AgentConfig,
    message_id: String,
    content: &str,
    finish_reason: Option<String>,
) -> SpeakerResult {
    SpeakerResult::Completed(ChatMessage {
        id: message_id,
        role: "assistant".to_string(),
        name: Some(speaker.name.clone()),
        content: content.to_string(),
        timestamp: crate::vcp_modules::infra::utils::now_millis() as u64,
        updated_at: Some(crate::vcp_modules::infra::utils::now_millis() as u64),
        is_thinking: Some(false),
        agent_id: Some(speaker.id.clone()),
        group_id: Some(group_id.to_string()),
        topic_id: Some(topic_id.to_string()),
        is_group_message: Some(true),
        finish_reason,
        ..Default::default()
    })
}

async fn finalize_failure(
    error: VcpRequestError,
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    stream_channel: Option<Channel<StreamEvent>>,
) -> Result<SpeakerResult, String> {
    let VcpRequestError {
        message,
        completion_lease,
        stale,
        request_guard,
    } = error;
    let _request_guard = request_guard;
    if stale {
        return Ok(SpeakerResult::Skipped);
    }
    let Some(lease) = completion_lease else {
        log::error!("[GroupChatAppService] 群聊请求失败: {message}");
        return Ok(SpeakerResult::Failed);
    };
    let mark = mark_message_as_error_guarded_with_channel(
        app,
        pool,
        &lease,
        Some(message.clone()),
        stream_channel,
    )
    .await?;
    match mark {
        GuardedTransition::Applied(()) => {
            log::error!("[GroupChatAppService] 群聊请求失败: {message}");
            Ok(SpeakerResult::Failed)
        }
        GuardedTransition::Skipped => Ok(SpeakerResult::Skipped),
    }
}
