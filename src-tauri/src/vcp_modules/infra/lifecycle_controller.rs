use crate::vcp_modules::infra::lifecycle_state::LifecycleState;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager};

#[path = "lifecycle_transition.rs"]
mod transition;

pub fn is_app_in_foreground<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<LifecycleState>()
        .map(|state| state.is_foreground.load(Ordering::SeqCst))
        .unwrap_or(true)
}

#[tauri::command]
pub async fn set_app_foreground_state(
    app: AppHandle,
    is_foreground: bool,
    request_epoch: Option<u64>,
) {
    set_app_foreground_state_internal(app, is_foreground, request_epoch).await;
}

pub async fn set_app_foreground_state_internal(
    app: AppHandle,
    is_foreground: bool,
    request_epoch: Option<u64>,
) {
    transition::set_app_foreground_state_internal(app, is_foreground, request_epoch).await;
}
