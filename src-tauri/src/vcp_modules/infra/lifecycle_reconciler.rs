use crate::vcp_modules::infra::lifecycle_manager;
use crate::vcp_modules::infra::lifecycle_state::LifecycleState;
use tauri::AppHandle;

/// 根据设置决定启动或停止划词助手本地服务器
pub async fn reconcile_local_server(
    app_handle: &AppHandle,
    lifecycle: &LifecycleState,
    enable_assistant: bool,
) {
    lifecycle_manager::reconcile_local_server(app_handle, lifecycle, enable_assistant).await;
}

/// 根据设置决定启动或停止分布式节点连接
pub async fn reconcile_distributed_node(
    app_handle: &AppHandle,
    distributed_enabled: bool,
    force_reconnect: bool,
) {
    lifecycle_manager::reconcile_distributed_node(app_handle, distributed_enabled, force_reconnect)
        .await;
}
