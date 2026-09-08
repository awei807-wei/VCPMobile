// distributed/mod.rs
// Self-contained distributed node module.
// Does NOT depend on any vcp_modules/ code.
// To remove: delete this directory + 3 references in lib.rs → cargo check passes.

pub mod client;
pub mod telemetry_center;
pub mod tool_registry;
pub mod tools;
pub mod types;

use std::sync::Arc;

use client::DistributedClient;
use tauri::State;
use tokio::sync::RwLock;

/// Managed state for the distributed node. Registered via app.manage() in lib.rs.
pub struct DistributedState {
    pub client: RwLock<DistributedClient>,
    pub registry: Arc<tool_registry::ToolRegistry>,
    pub telemetry: Arc<telemetry_center::TelemetryCenter>,
}

impl DistributedState {
    pub fn new() -> Self {
        let registry = Arc::new(tools::build_registry());
        Self {
            client: RwLock::new(DistributedClient::new()),
            registry,
            telemetry: Arc::new(telemetry_center::TelemetryCenter::new()),
        }
    }
}

// ============================================================
// Tauri commands — entry points registered in lib.rs
// ============================================================

/// Get current distributed node status.
#[tauri::command]
pub async fn get_distributed_status(
    state: State<'_, DistributedState>,
) -> Result<types::DistributedStatus, String> {
    let client = state.client.read().await;
    Ok(client.get_status().await)
}

/// 获取已注册工具 metadata，并先加载安全 allowlist。
#[tauri::command]
pub async fn get_registered_tools_metadata(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
) -> Result<Vec<serde_json::Value>, String> {
    state.registry.get_tools_metadata(&app).await
}

/// 更新显式 enabled allowlist，并在成功落盘后通知远端重注册。
#[tauri::command]
pub async fn update_enabled_tools(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
    enabled_names: Vec<String>,
) -> Result<(), String> {
    if state.registry.update_enabled(&app, enabled_names).await? {
        let client = state.client.read().await;
        if client.is_connected().await {
            client.re_register_tools().await;
        }
    }
    Ok(())
}

/// 获取 allowlist 加载状态和最近错误。
#[tauri::command]
pub async fn get_distributed_tool_config_status(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
) -> Result<tool_registry::ToolConfigStatus, String> {
    Ok(state.registry.config_status(&app).await)
}

/// 安全清空旧 disabled 配置，并重置为空 enabled allowlist。
#[tauri::command]
pub async fn reset_distributed_tools_disabled(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
) -> Result<(), String> {
    state.registry.reset_enabled(&app).await?;
    let client = state.client.read().await;
    if client.is_connected().await {
        client.re_register_tools().await;
    }
    Ok(())
}

/// Execute a distributed tool by name.
#[tauri::command]
pub async fn execute_distributed_tool(
    app: tauri::AppHandle,
    state: State<'_, DistributedState>,
    name: String,
    args: Option<serde_json::Value>,
) -> Result<String, String> {
    let res = state
        .registry
        .execute(&name, args.unwrap_or(serde_json::Value::Null), &app)
        .await?;
    match res {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

/// Trigger immediate reconnect of the distributed client.
#[tauri::command]
pub async fn reconnect_distributed_client(
    state: State<'_, DistributedState>,
) -> Result<(), String> {
    let client = state.client.read().await;
    client.trigger_reconnect().await;
    Ok(())
}
