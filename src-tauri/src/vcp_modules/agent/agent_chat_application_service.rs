use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter, State};

#[path = "agent_chat_application_service/agent_chat.rs"]
mod agent_chat;
#[path = "agent_chat_application_service/assistant_chat.rs"]
mod assistant_chat;

pub use agent_chat::{
    handle_agent_chat_message, internal_process_agent_chat_message, AgentChatPayload,
};
pub use assistant_chat::{handle_assistant_chat_stream, AssistantChatPayload};

#[derive(Clone, Default)]
pub struct AssistantChatActivityState {
    active_count: Arc<AtomicUsize>,
}

impl AssistantChatActivityState {
    fn emit(&self, app_handle: &AppHandle, active_count: usize) {
        let _ = app_handle.emit(
            "floating-assistant-activity",
            json!({
                "activeCount": active_count,
                "isGenerating": active_count > 0,
            }),
        );
    }

    pub fn begin(&self, app_handle: &AppHandle) -> AssistantChatActivityGuard {
        let active_count = self.active_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.emit(app_handle, active_count);
        AssistantChatActivityGuard {
            state: self.clone(),
            app_handle: app_handle.clone(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_count.load(Ordering::SeqCst) > 0
    }
}

pub struct AssistantChatActivityGuard {
    state: AssistantChatActivityState,
    app_handle: AppHandle,
}

impl Drop for AssistantChatActivityGuard {
    fn drop(&mut self) {
        let previous = self.state.active_count.fetch_sub(1, Ordering::SeqCst);
        self.state
            .emit(&self.app_handle, previous.saturating_sub(1));
    }
}

#[tauri::command]
pub async fn is_assistant_chat_active(
    activity_state: State<'_, AssistantChatActivityState>,
) -> Result<bool, String> {
    Ok(activity_state.is_active())
}
