use super::{reconcile_distributed_node, reconcile_local_server};
use crate::vcp_modules::db_manager::{init_db, DbState};
use crate::vcp_modules::infra::lifecycle_state::{CoreStatus, LifecycleState};
use crate::vcp_modules::settings_manager::{read_settings, Settings, SettingsState};
use crate::vcp_modules::sync_service::init_sync_service;
use crate::vcp_modules::vcp_client::cleanup_recovery_cleanup_debt_on_startup;
use log::info;
use tauri::{AppHandle, Emitter, Manager};

#[path = "lifecycle_bootstrap_tasks.rs"]
mod tasks;

pub async fn bootstrap(app: &AppHandle) -> Result<(), String> {
    let lifecycle = app.state::<LifecycleState>();
    let handle = app.clone();
    info!("[Lifecycle] 开始核心启动流程。");
    emit_core_event(&handle, "initializing", "核心引擎初始化中...");
    init_database(&handle, &lifecycle).await?;
    run_recovery_cleanup_startup(&handle).await;
    let settings = match load_settings(&handle).await {
        Ok(settings) => settings,
        Err(error) => {
            persist_bootstrap_failure(&handle, &lifecycle, error.clone()).await;
            return Err(error);
        }
    };
    reconcile_services(&handle, &lifecycle, &settings).await;

    handle.manage(init_sync_service(handle.clone()));
    tasks::spawn_background_services(
        &handle,
        settings.vcp_log_url.clone(),
        settings.vcp_log_key.clone(),
    );

    *lifecycle.status.write().await = CoreStatus::Ready;
    emit_core_event(&handle, "ready", "核心引擎已就绪");
    info!("[Lifecycle] 核心启动完成，状态为 READY。");
    tasks::spawn_network_monitoring(&handle);
    tasks::spawn_frontend_update_check(&handle);
    Ok(())
}

async fn run_recovery_cleanup_startup(handle: &AppHandle) {
    let Some(db_state) = handle.try_state::<DbState>() else {
        log::error!("[Lifecycle] 数据库就绪后未找到 DbState，跳过恢复清理扫描。");
        return;
    };
    let cache_dir = match handle.path().app_cache_dir() {
        Ok(path) => path,
        Err(error) => {
            log::error!(
                "[Lifecycle] 无法解析 app cache 路径，跳过恢复清理扫描：{}",
                error
            );
            return;
        }
    };
    cleanup_recovery_cleanup_debt_on_startup(&cache_dir, &db_state.pool).await;
}

fn emit_core_event(handle: &AppHandle, status: &str, message: &str) {
    let _ = handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": status,
            "message": message,
            "source": "Core"
        }),
    );
}

async fn init_database(handle: &AppHandle, lifecycle: &LifecycleState) -> Result<(), String> {
    match init_db(handle).await {
        Ok((pool, path)) => {
            handle.manage(DbState { pool, path });
            Ok(())
        }
        Err(error) => {
            let message = format!("数据库初始化失败：{}", error);
            *lifecycle.last_error.write().await = Some(message.clone());
            *lifecycle.status.write().await = CoreStatus::Error;
            *lifecycle.status_message.write().await = message.clone();
            emit_core_event(handle, "error", &message);
            Err(message)
        }
    }
}

async fn load_settings(handle: &AppHandle) -> Result<Settings, String> {
    let settings_state = handle.state::<SettingsState>();
    read_settings(handle.clone(), settings_state)
        .await
        .map_err(|error| format!("基础配置读取失败：{}", error))
}

async fn persist_bootstrap_failure(
    handle: &AppHandle,
    lifecycle: &LifecycleState,
    message: String,
) {
    *lifecycle.last_error.write().await = Some(message.clone());
    *lifecycle.status.write().await = CoreStatus::Error;
    *lifecycle.status_message.write().await = message.clone();
    emit_core_event(handle, "error", &message);
}

async fn reconcile_services(handle: &AppHandle, lifecycle: &LifecycleState, settings: &Settings) {
    log::info!(
        "[Lifecycle] enableAssistant={}，调和本地服务。",
        settings.enable_assistant
    );
    reconcile_local_server(handle, lifecycle, settings.enable_assistant).await;
    log::info!(
        "[Lifecycle] distributedEnabled={}，调和分布式节点。",
        settings.distributed_enabled
    );
    reconcile_distributed_node(handle, settings.distributed_enabled, false).await;
}
