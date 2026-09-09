use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::emoticon_manager::{
    internal_load_library, refresh_emoticon_library_internal, EmoticonManagerState,
};
use crate::vcp_modules::model_manager::{init_model_manager, ModelManagerState};
use crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal;
use log::info;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub(super) fn spawn_background_services(
    handle: &AppHandle,
    vcp_log_url: String,
    vcp_log_key: String,
) {
    let app = handle.clone();
    tokio::spawn(initialize_optional_services(app, vcp_log_url, vcp_log_key));

    let app = handle.clone();
    tokio::spawn(initialize_model_manager_task(app));
    spawn_delete_executor_cleanup(handle);
}

async fn initialize_optional_services(app: AppHandle, vcp_log_url: String, vcp_log_key: String) {
    let emoticon_state = app.state::<EmoticonManagerState>();
    if let Ok(library) = internal_load_library(&app).await {
        *emoticon_state.library.lock().await = library;
        info!("[Lifecycle] 已从数据库加载表情包库。");
    }
    match refresh_emoticon_library_internal(&app, false).await {
        Ok(count) => info!("[Lifecycle] 表情包库已刷新：{} 项。", count),
        Err(error) => info!("[Lifecycle] 跳过表情包自动刷新：{}", error),
    }

    if !vcp_log_url.is_empty() && !vcp_log_key.is_empty() {
        info!("[Lifecycle] 正在自动连接 VCP Log。");
        let _ =
            init_vcp_log_connection_internal(app.clone(), vcp_log_url.clone(), vcp_log_key.clone())
                .await;
        info!("[Lifecycle] 正在自动连接 VCP Info。");
        let _ = crate::vcp_modules::vcp_info_service::init_vcp_info_connection(
            app,
            vcp_log_url,
            vcp_log_key,
        )
        .await;
    }
}

async fn initialize_model_manager_task(app: AppHandle) {
    let model_state = app.state::<ModelManagerState>();
    init_model_manager(&app, &model_state).await;
    info!("[Lifecycle] 模型管理器已在后台初始化。");
}

fn spawn_delete_executor_cleanup(handle: &AppHandle) {
    tokio::spawn(run_delete_executor_cleanup(handle.clone()));
}

async fn run_delete_executor_cleanup(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(10)).await;
    let pool = &app.state::<DbState>().pool;
    if !check_ran_recently(pool, "delete_executor_last_cleanup").await {
        perform_delete_cleanup(&app, pool).await;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(86400)).await;
        perform_delete_cleanup(&app, pool).await;
    }
}

async fn perform_delete_cleanup(app: &AppHandle, pool: &sqlx::SqlitePool) {
    use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
    if DeleteExecutor::cleanup_old_deleted_records(app, 30)
        .await
        .is_ok()
    {
        record_check(pool, "delete_executor_last_cleanup").await;
    }
}

async fn check_ran_recently(pool: &sqlx::SqlitePool, key: &str) -> bool {
    use sqlx::Row;
    let Some(row) = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    else {
        return false;
    };
    let Ok(last_check) = row.get::<String, _>("value").parse::<i64>() else {
        return false;
    };
    crate::vcp_modules::infra::utils::now_millis() - last_check < 86_400_000
}

async fn record_check(pool: &sqlx::SqlitePool, key: &str) {
    let now = crate::vcp_modules::infra::utils::now_millis();
    let _ =
        sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (?, ?, ?)")
            .bind(key)
            .bind(now.to_string())
            .bind(now)
            .execute(pool)
            .await;
}

pub(super) fn spawn_network_monitoring(handle: &AppHandle) {
    tokio::spawn(start_network_monitoring(handle.clone()));
}

async fn start_network_monitoring(app: AppHandle) {
    info!("[Lifecycle] 正在激活原生网络状态监听。");
    if let Err(error) = tauri_plugin_vcp_mobile::system::start_network_monitoring(app) {
        log::error!("[Lifecycle] 启动原生网络状态监听失败：{}", error);
    }
}

pub(super) fn spawn_frontend_update_check(handle: &AppHandle) {
    tokio::spawn(run_frontend_update_check(handle.clone()));
}

async fn run_frontend_update_check(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    let pool = &app.state::<DbState>().pool;
    if check_ran_recently(pool, "frontend_update_last_check").await {
        return;
    }
    info!("[FrontendUpdate] 开始后台检查。");
    match crate::vcp_modules::frontend_update_manager::check_for_frontend_update(app.clone()).await
    {
        Ok(update) => {
            record_check(pool, "frontend_update_last_check").await;
            apply_frontend_update(&app, update).await;
        }
        Err(error) => log::error!("[FrontendUpdate] 检查失败：{}", error),
    }
}

async fn apply_frontend_update(
    app: &AppHandle,
    update: crate::vcp_modules::frontend_update_manager::FrontendUpdateInfo,
) {
    let Some(url) = update.download_url else {
        info!("[FrontendUpdate] 当前没有可用更新。");
        return;
    };
    info!(
        "[FrontendUpdate] 发现新版本 {}，开始下载。",
        update.remote_version
    );
    match crate::vcp_modules::frontend_update_manager::download_frontend_update_inner(
        app, &url, None,
    )
    .await
    {
        Ok(zip_path) => {
            if let Err(error) = crate::vcp_modules::frontend_update_manager::apply_frontend_update(
                app.clone(),
                zip_path,
                update.remote_version.clone(),
            )
            .await
            {
                log::error!("[FrontendUpdate] 应用更新失败：{}", error);
            } else {
                info!(
                    "[FrontendUpdate] 版本 {} 已下载并应用，下次冷启动生效。",
                    update.remote_version
                );
            }
        }
        Err(error) => log::error!("[FrontendUpdate] 下载失败：{}", error),
    }
}
