use tauri::{AppHandle, Manager};

use crate::distributed::client::DistributedClient;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::infra::lifecycle_state::LifecycleState;
use crate::vcp_modules::infra::local_server;
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
use std::sync::Arc;

#[path = "lifecycle_bootstrap.rs"]
mod bootstrap;
#[path = "lifecycle_commands.rs"]
mod commands;

pub use bootstrap::bootstrap;
pub use commands::{
    get_core_status, get_last_error, get_system_snapshot, reconcile_distributed_node_cmd,
    reconcile_local_server_cmd,
};

struct DistributedConnectionConfig {
    ws_url: String,
    vcp_key: String,
    device_name: String,
}

pub fn is_app_in_foreground<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<LifecycleState>()
        .map(|state| {
            state
                .is_foreground
                .load(std::sync::atomic::Ordering::Relaxed)
        })
        .unwrap_or(true)
}

pub async fn reconcile_local_server(
    app_handle: &AppHandle,
    lifecycle: &LifecycleState,
    enable_assistant: bool,
) {
    let mut handle_lock = lifecycle.local_server_handle.lock().await;
    match (enable_assistant, handle_lock.is_some()) {
        (true, false) => {
            log::info!("[Lifecycle] 启用划词助手，启动本地服务。");
            *handle_lock = Some(local_server::start_server(app_handle.clone()));
        }
        (false, true) => {
            log::info!("[Lifecycle] 停用划词助手，停止本地服务。");
            if let Some(handle) = handle_lock.take() {
                handle.shutdown().await;
            }
        }
        _ => {}
    }
}

pub async fn reconcile_distributed_node(
    app_handle: &AppHandle,
    distributed_enabled: bool,
    force_reconnect: bool,
) {
    let Some(distributed_state) = app_handle.try_state::<crate::distributed::DistributedState>()
    else {
        log::warn!("[Lifecycle] DistributedState 未注册，跳过调和。");
        return;
    };
    let client = distributed_state.client.read().await;
    if !distributed_enabled {
        stop_distributed_client(&client, app_handle, None).await;
        if let Err(error) =
            tauri_plugin_vcp_mobile::stream::release_distributed_keepalive_inner(app_handle)
        {
            log::warn!("[Lifecycle] 清理原生分布式恢复状态失败：{}", error);
        }
        return;
    }

    reconcile_enabled_distributed(
        app_handle,
        &client,
        distributed_state.registry.clone(),
        force_reconnect,
        false,
        "自动连接",
    )
    .await;
}

async fn reconcile_enabled_distributed(
    app_handle: &AppHandle,
    client: &DistributedClient,
    registry: Arc<crate::distributed::tool_registry::ToolRegistry>,
    force_reconnect: bool,
    trigger_reconnect: bool,
    source: &str,
) {
    let request_id = client.reserve_start_request();
    let Some(config) = load_enabled_connection_config(app_handle, client, source).await else {
        return;
    };
    log::info!("[Lifecycle] 分布式已启用，开始调和节点连接。");
    if let Err(error) = client
        .reconcile_with_request(
            request_id,
            app_handle,
            true,
            force_reconnect,
            trigger_reconnect,
            config.ws_url,
            config.vcp_key,
            config.device_name,
            registry,
        )
        .await
    {
        log::error!("[Lifecycle] {}调和分布式节点失败：{}", source, error);
    }
}

async fn load_enabled_connection_config(
    app_handle: &AppHandle,
    client: &DistributedClient,
    source: &str,
) -> Option<DistributedConnectionConfig> {
    let settings_state = match app_handle.try_state::<SettingsState>() {
        Some(state) => state,
        None => {
            stop_distributed_client(
                client,
                app_handle,
                Some(format!("分布式{}时 SettingsState 未注册", source)),
            )
            .await;
            return None;
        }
    };
    let settings = match read_settings(app_handle.clone(), settings_state).await {
        Ok(settings) => settings,
        Err(error) => {
            log::error!(
                "[Lifecycle] 读取分布式{}连接设置失败：{}，停止现有连接。",
                source,
                error
            );
            stop_distributed_client(client, app_handle, Some(error.to_string())).await;
            return None;
        }
    };
    if !settings.distributed_enabled {
        log::info!("[Lifecycle] 权威设置已禁用分布式，取消本次启动并确保节点停止。");
        stop_distributed_client(client, app_handle, None).await;
        return None;
    }
    match build_connection_config(&settings, source) {
        Ok(config) => Some(config),
        Err(error) => {
            log::error!("[Lifecycle] {}，停止现有连接。", error);
            stop_distributed_client(client, app_handle, Some(error)).await;
            None
        }
    }
}

async fn stop_distributed_client(
    client: &DistributedClient,
    app_handle: &AppHandle,
    reason: Option<String>,
) {
    let stop_request = client.reserve_stop_request();
    client
        .stop_with_request_reason(stop_request, app_handle, reason)
        .await;
}

/// 供前后台 transition 在配置缺失或失效时执行 fail-closed 停止。
pub async fn stop_distributed_with_reason(app_handle: &AppHandle, reason: impl Into<String>) {
    let Some(distributed_state) = app_handle.try_state::<crate::distributed::DistributedState>()
    else {
        log::warn!("[Lifecycle] DistributedState 未注册，无法执行 fail-closed 停止。");
        return;
    };
    let client = distributed_state.client.read().await;
    stop_distributed_client(&client, app_handle, Some(reason.into())).await;
}

pub async fn recover_distributed_node_after_network_restore(app_handle: &AppHandle) {
    let Some(distributed_state) = app_handle.try_state::<crate::distributed::DistributedState>()
    else {
        log::warn!("[Lifecycle] DistributedState 未注册，跳过网络恢复。");
        return;
    };
    let client = distributed_state.client.read().await;
    if app_handle.try_state::<DbState>().is_none() {
        log::info!("[Lifecycle] 数据库尚未注册，停止现有连接并延后网络恢复。");
        stop_distributed_client(
            &client,
            app_handle,
            Some("网络恢复时数据库尚未注册，停止现有分布式连接".to_string()),
        )
        .await;
        return;
    }
    log::info!("[Distributed] 网络恢复，调和并唤醒分布式连接。");
    reconcile_enabled_distributed(
        app_handle,
        &client,
        distributed_state.registry.clone(),
        false,
        true,
        "网络恢复",
    )
    .await;
}

fn build_connection_config(
    settings: &Settings,
    source: &str,
) -> Result<DistributedConnectionConfig, String> {
    let ws_url = settings.distributed_ws_url.trim();
    let vcp_key = settings.distributed_vcp_key.trim();
    if ws_url.is_empty() || vcp_key.is_empty() {
        return Err(format!(
            "分布式{}时连接地址或密钥为空，拒绝继续保持连接",
            source
        ));
    }
    let parsed_url = url::Url::parse(ws_url.trim_end_matches('/'))
        .map_err(|error| format!("分布式{}时连接地址无效：{}", source, error))?;
    if !matches!(parsed_url.scheme(), "ws" | "wss" | "http" | "https") {
        return Err(format!(
            "分布式{}时连接地址协议无效：{}",
            source,
            parsed_url.scheme()
        ));
    }
    if parsed_url
        .host_str()
        .is_none_or(|host| host.trim().is_empty())
    {
        return Err(format!("分布式{}时连接地址缺少主机名", source));
    }
    Ok(DistributedConnectionConfig {
        ws_url: ws_url.to_string(),
        vcp_key: vcp_key.to_string(),
        device_name: if settings.distributed_device_name.trim().is_empty() {
            "VCPMobile".to_string()
        } else {
            settings.distributed_device_name.trim().to_string()
        },
    })
}

pub(crate) fn validate_distributed_connection_config(
    settings: &Settings,
    source: &str,
) -> Result<(), String> {
    build_connection_config(settings, source).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{build_connection_config, Settings};

    #[test]
    fn 空地址或密钥拒绝连接配置() {
        let mut settings = Settings::default();
        settings.distributed_ws_url = "ws://127.0.0.1:5800".to_string();
        assert!(build_connection_config(&settings, "测试").is_err());
        settings.distributed_vcp_key = "密钥".to_string();
        settings.distributed_ws_url.clear();
        assert!(build_connection_config(&settings, "测试").is_err());
    }

    #[test]
    fn 无效地址协议拒绝连接配置() {
        let settings = Settings {
            distributed_enabled: true,
            distributed_ws_url: "ftp://127.0.0.1:5800".to_string(),
            distributed_vcp_key: "密钥".to_string(),
            ..Settings::default()
        };
        assert!(build_connection_config(&settings, "测试").is_err());
    }

    #[test]
    fn 缺少主机名拒绝连接配置() {
        let settings = Settings {
            distributed_enabled: true,
            distributed_ws_url: "ws://:5800".to_string(),
            distributed_vcp_key: "密钥".to_string(),
            ..Settings::default()
        };
        assert!(build_connection_config(&settings, "测试").is_err());
    }

    #[test]
    fn 空白地址或密钥拒绝连接配置() {
        let settings = Settings {
            distributed_enabled: true,
            distributed_ws_url: "  ws://127.0.0.1:5800  ".to_string(),
            distributed_vcp_key: "   ".to_string(),
            ..Settings::default()
        };
        assert!(build_connection_config(&settings, "测试").is_err());

        let settings = Settings {
            distributed_enabled: true,
            distributed_ws_url: "   ".to_string(),
            distributed_vcp_key: "密钥".to_string(),
            ..Settings::default()
        };
        assert!(build_connection_config(&settings, "测试").is_err());
    }
}
