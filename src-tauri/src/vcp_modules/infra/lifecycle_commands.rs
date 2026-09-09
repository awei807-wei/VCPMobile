use super::{reconcile_distributed_node, reconcile_local_server};
use crate::vcp_modules::infra::lifecycle_state::{CoreStatus, LifecycleState};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Serialize, Clone)]
pub struct SystemSnapshot {
    pub core: CoreStatus,
    pub log: String,
    pub sync: String,
    pub distributed: String,
}

#[tauri::command]
pub async fn get_system_snapshot(
    state: State<'_, LifecycleState>,
    app: AppHandle,
) -> Result<SystemSnapshot, String> {
    let core = *state.status.read().await;

    // 获取 VCPLog 状态
    let log = crate::vcp_modules::vcp_log_service::get_vcp_log_status_internal().await;

    // 获取 Sync 状态
    let sync = match app.try_state::<crate::vcp_modules::sync_service::SyncState>() {
        Some(s) => s.connection_status.read().await.clone(),
        None => "closed".to_string(),
    };

    // 获取分布式连接状态
    let distributed = match app.try_state::<crate::distributed::DistributedState>() {
        Some(s) => {
            let client = s.client.read().await;
            let status = client.get_status().await;
            serde_json::to_value(status.state)
                .unwrap_or_else(|_| serde_json::json!("disconnected"))
                .as_str()
                .unwrap_or("disconnected")
                .to_string()
        }
        None => "disconnected".to_string(),
    };

    Ok(SystemSnapshot {
        core,
        log,
        sync,
        distributed,
    })
}

/// 前端保存设置后调用，即时生效启用/停用划词助手本地服务器
#[tauri::command]
pub async fn reconcile_local_server_cmd(
    app_handle: AppHandle,
    state: State<'_, LifecycleState>,
    enable: bool,
) -> Result<bool, String> {
    log::info!(
        "[Lifecycle] reconcile_local_server_cmd called: enable={}",
        enable
    );
    let lifecycle = &*state;
    reconcile_local_server(&app_handle, lifecycle, enable).await;
    Ok(enable)
}

#[tauri::command]
pub async fn reconcile_distributed_node_cmd(
    app_handle: AppHandle,
    enable: bool,
) -> Result<bool, String> {
    log::info!(
        "[Lifecycle] reconcile_distributed_node_cmd called: enable={}",
        enable
    );
    reconcile_distributed_node(&app_handle, enable, false).await;
    Ok(enable)
}

#[tauri::command]
pub async fn get_core_status(state: State<'_, LifecycleState>) -> Result<CoreStatus, String> {
    Ok(*state.status.read().await)
}

#[tauri::command]
pub async fn get_last_error(state: State<'_, LifecycleState>) -> Result<Option<String>, String> {
    Ok(state.last_error.read().await.clone())
}
