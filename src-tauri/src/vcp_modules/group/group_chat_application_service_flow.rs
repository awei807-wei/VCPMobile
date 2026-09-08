use super::GroupChatParams;
use crate::vcp_modules::agent_service::AgentConfigState;
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_context_assembler::assemble_group_context;
use crate::vcp_modules::group_service::GroupManagerState;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::vcp_client::{
    acquire_stream_service, perform_vcp_request, ActiveRequests, CancelledGroupTurns, StreamEvent,
    VcpRequestPayload,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{ipc::Channel, AppHandle, Emitter, State};
use tauri_plugin_vcp_mobile::stream::StreamIdentity;

#[path = "group_chat_application_service_flow_setup.rs"]
mod setup;
use setup::TurnSetup;

#[path = "group_chat_application_service_flow_finalize.rs"]
mod finalize;

struct TurnSummary {
    messages: Vec<ChatMessage>,
    failures: usize,
    skipped: usize,
}

enum SpeakerResult {
    Completed(ChatMessage),
    Failed,
    Skipped,
}

#[allow(clippy::too_many_arguments)]
pub async fn process_group_chat_message(
    app_handle: AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: State<'_, AgentConfigState>,
    db_state: State<'_, DbState>,
    active_requests: State<'_, ActiveRequests>,
    cancelled_turns: State<'_, CancelledGroupTurns>,
    params: GroupChatParams,
    append_user_msg: bool,
) -> Result<Value, String> {
    let GroupChatParams {
        group_id,
        topic_id,
        user_message,
        vcp_url,
        vcp_api_key,
        stream_channel,
    } = params;
    cancelled_turns.0.remove(&topic_id);
    let setup = setup::load_turn_setup(
        &app_handle,
        group_state,
        &agent_state,
        &db_state,
        &group_id,
        &topic_id,
        &user_message,
        append_user_msg,
    )
    .await?;
    let Some(setup) = setup else {
        return Ok(json!({"status": "no_ai_response"}));
    };
    let summary = process_speakers(
        &app_handle,
        &db_state.pool,
        &active_requests.0,
        &cancelled_turns.0,
        &group_id,
        &topic_id,
        &user_message,
        &vcp_url,
        &vcp_api_key,
        stream_channel.clone(),
        setup,
    )
    .await?;
    emit_group_finished(&app_handle, &group_id, &topic_id, &summary)?;
    cancelled_turns.0.remove(&topic_id);
    Ok(summary_status(summary))
}

#[allow(clippy::too_many_arguments)]
async fn process_speakers(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    active_requests: &Arc<crate::vcp_modules::vcp_client::ActiveRequestRegistry>,
    cancelled_turns: &Arc<dashmap::DashSet<String>>,
    group_id: &str,
    topic_id: &str,
    user_message: &ChatMessage,
    vcp_url: &str,
    vcp_api_key: &str,
    stream_channel: Option<Channel<StreamEvent>>,
    setup: TurnSetup,
) -> Result<TurnSummary, String> {
    let mut history = setup.history.clone();
    let mut messages = Vec::new();
    let mut failures = 0;
    let mut skipped = 0;
    for speaker in &setup.speakers {
        if cancelled_turns.contains(topic_id) {
            break;
        }
        match process_speaker(
            app,
            pool,
            active_requests,
            group_id,
            topic_id,
            user_message,
            vcp_url,
            vcp_api_key,
            stream_channel.clone(),
            &setup,
            &history,
            speaker.clone(),
        )
        .await?
        {
            SpeakerResult::Completed(message) => {
                history.push(message.clone());
                messages.push(message);
            }
            SpeakerResult::Failed => failures += 1,
            SpeakerResult::Skipped => skipped += 1,
        }
    }
    Ok(TurnSummary {
        messages,
        failures,
        skipped,
    })
}

#[allow(clippy::too_many_arguments)]
async fn process_speaker(
    app: &AppHandle,
    pool: &sqlx::Pool<sqlx::Sqlite>,
    active_requests: &Arc<crate::vcp_modules::vcp_client::ActiveRequestRegistry>,
    group_id: &str,
    topic_id: &str,
    user_message: &ChatMessage,
    vcp_url: &str,
    vcp_api_key: &str,
    stream_channel: Option<Channel<StreamEvent>>,
    setup: &TurnSetup,
    history: &[ChatMessage],
    speaker: AgentConfig,
) -> Result<SpeakerResult, String> {
    let message_id = format!(
        "msg_group_{}_{}_{}",
        user_message.id,
        speaker.id,
        crate::vcp_modules::infra::utils::now_millis()
    );
    let identity = StreamIdentity::new("group", group_id, topic_id, &message_id);
    let stream_lease = acquire_stream_service(app, &speaker.name, &identity, "GroupChatAppService");
    let request = build_speaker_request(
        pool,
        group_id,
        topic_id,
        vcp_url,
        vcp_api_key,
        setup,
        history,
        &speaker,
        message_id.clone(),
    )
    .await?;
    let result = perform_vcp_request(
        app,
        active_requests.clone(),
        request,
        stream_channel.clone(),
    )
    .await;
    let finalized = finalize::finalize_speaker_result(
        result,
        app,
        pool,
        group_id,
        topic_id,
        &speaker,
        message_id,
        stream_channel,
    )
    .await;
    drop(stream_lease);
    finalized
}

#[allow(clippy::too_many_arguments)]
async fn build_speaker_request(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    group_id: &str,
    topic_id: &str,
    vcp_url: &str,
    vcp_api_key: &str,
    setup: &TurnSetup,
    history: &[ChatMessage],
    speaker: &AgentConfig,
    message_id: String,
) -> Result<VcpRequestPayload, String> {
    let system_prompt =
        assemble_group_context(speaker, &setup.group_config, &setup.active_members).await;
    let model = choose_model(&setup.group_config, speaker);
    let mut model_config = json!({"model": model, "stream": true});
    if speaker.use_temperature {
        model_config["temperature"] = json!(speaker.temperature);
    }
    let invite = setup
        .group_config
        .invite_prompt
        .as_ref()
        .map(|prompt| prompt.replace("{{VCPChatAgentName}}", &speaker.name));
    let messages = crate::vcp_modules::context_assembler::orchestrate_chat_context(
        crate::vcp_modules::context_assembler::ChatContextRequest {
            pool,
            history,
            owner_id: group_id,
            topic_id,
            agent_name: &speaker.name,
            scope: "group",
            base_system_prompt: system_prompt,
            invite_prompt: invite,
        },
    )
    .await?;
    Ok(VcpRequestPayload {
        vcp_url: vcp_url.to_string(),
        vcp_api_key: vcp_api_key.to_string(),
        messages,
        model_config,
        message_id: message_id.clone(),
        context: Some(json!({
            "groupId": group_id,
            "ownerType": "group",
            "topicId": topic_id,
            "speakerAgentId": speaker.id,
            "isGroupMessage": true,
            "agentName": speaker.name
        })),
    })
}

fn choose_model(group_config: &GroupConfig, speaker: &AgentConfig) -> String {
    if group_config.use_unified_model {
        if let Some(model) = group_config
            .unified_model
            .as_ref()
            .filter(|model| !model.is_empty())
        {
            return model.clone();
        }
    }
    speaker.model.clone()
}

fn emit_group_finished(
    app: &AppHandle,
    group_id: &str,
    topic_id: &str,
    summary: &TurnSummary,
) -> Result<(), String> {
    let agent_ids = summary
        .messages
        .iter()
        .filter_map(|message| message.agent_id.clone())
        .collect::<Vec<_>>();
    app.emit(
        "vcp-group-turn-finished",
        json!({
            "groupId": group_id,
            "topic_id": topic_id,
            "agentIds": agent_ids,
            "failedCount": summary.failures,
            "skippedCount": summary.skipped
        }),
    )
    .map_err(|error| format!("发送群聊回合结束事件失败: {error}"))
}

fn summary_status(summary: TurnSummary) -> Value {
    let status = if summary.messages.is_empty() {
        "failed"
    } else if summary.failures > 0 || summary.skipped > 0 {
        "partial"
    } else {
        "completed"
    };
    json!({
        "status": status,
        "successCount": summary.messages.len(),
        "failedCount": summary.failures,
        "skippedCount": summary.skipped
    })
}
