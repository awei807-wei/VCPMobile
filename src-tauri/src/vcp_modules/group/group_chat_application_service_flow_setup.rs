use crate::vcp_modules::agent_service::{read_agent_config_internal, AgentConfigState};
use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::group_service::{read_group_config, GroupManagerState};
use crate::vcp_modules::group_speaking_policy::determine_naturerandom_speakers;
use crate::vcp_modules::group_types::GroupConfig;
use crate::vcp_modules::message_service;
use tauri::{AppHandle, State};

pub(super) struct TurnSetup {
    pub(super) group_config: GroupConfig,
    pub(super) active_members: Vec<AgentConfig>,
    pub(super) speakers: Vec<AgentConfig>,
    pub(super) history: Vec<ChatMessage>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn load_turn_setup(
    app: &AppHandle,
    group_state: State<'_, GroupManagerState>,
    agent_state: &State<'_, AgentConfigState>,
    db_state: &DbState,
    group_id: &str,
    topic_id: &str,
    user_message: &ChatMessage,
    append_user_msg: bool,
) -> Result<Option<TurnSetup>, String> {
    let group_config = read_group_config(app.clone(), group_state, group_id.to_string()).await?;
    let active_members = load_group_members(app, agent_state, &group_config).await?;
    if append_user_msg {
        message_service::append_single_message(
            app.clone(),
            &db_state.pool,
            group_id,
            "group",
            topic_id.to_string(),
            user_message.clone(),
        )
        .await?;
    }
    let decision_history = load_history(app, group_id, topic_id, Some(8), false).await?;
    let speakers = choose_speakers(
        &group_config,
        &active_members,
        &decision_history,
        user_message,
    )?;
    if speakers.is_empty() {
        return Ok(None);
    }
    let history = load_history(app, group_id, topic_id, None, true).await?;
    Ok(Some(TurnSetup {
        group_config,
        active_members,
        speakers,
        history,
    }))
}

async fn load_group_members(
    app: &AppHandle,
    agent_state: &State<'_, AgentConfigState>,
    group_config: &GroupConfig,
) -> Result<Vec<AgentConfig>, String> {
    let mut members = Vec::with_capacity(group_config.members.len());
    for member_id in &group_config.members {
        let config = read_agent_config_internal(app, agent_state, member_id, Some(false)).await?;
        members.push(config);
    }
    Ok(members)
}

async fn load_history(
    app: &AppHandle,
    group_id: &str,
    topic_id: &str,
    limit: Option<usize>,
    include_extracted_text: bool,
) -> Result<Vec<ChatMessage>, String> {
    message_service::load_chat_history_internal(
        app,
        group_id,
        "group",
        topic_id,
        limit,
        None,
        true,
        include_extracted_text,
    )
    .await
}

fn choose_speakers(
    group_config: &GroupConfig,
    active_members: &[AgentConfig],
    history: &[ChatMessage],
    user_message: &ChatMessage,
) -> Result<Vec<AgentConfig>, String> {
    match group_config.mode.as_str() {
        "sequential" => Ok(active_members.to_vec()),
        "naturerandom" => Ok(determine_naturerandom_speakers(
            active_members,
            history,
            group_config,
            user_message,
        )),
        mode => Err(format!("群聊发言模式暂不支持: {mode}")),
    }
}
