use crate::vcp_modules::infra::lifecycle_state::{LifecycleState, LingerController};
use crate::vcp_modules::settings_manager::{read_settings, SettingsState};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub async fn set_app_foreground_state_internal(
    app: AppHandle,
    is_foreground: bool,
    requested_epoch: Option<u64>,
) {
    let state = match app.try_state::<LifecycleState>() {
        Some(state) => state,
        None => {
            log::warn!("[Lifecycle] LifecycleState 未注册，跳过前后台切换。");
            return;
        }
    };
    let _transition = state.transition.lock().await;
    let Some(epoch) = resolve_foreground_epoch(&state, requested_epoch, is_foreground) else {
        return;
    };
    state
        .foreground_request_epoch
        .store(epoch, Ordering::SeqCst);
    let was_foreground = state.is_foreground.swap(is_foreground, Ordering::SeqCst);
    if was_foreground == is_foreground {
        return;
    }
    apply_foreground_change(&app, &state, was_foreground, is_foreground, epoch).await;
}

fn resolve_foreground_epoch(
    state: &LifecycleState,
    requested_epoch: Option<u64>,
    is_foreground: bool,
) -> Option<u64> {
    let (epoch, is_implicit_request) = match requested_epoch {
        Some(0) => {
            log::warn!("[生命周期] 拒绝无效的前后台请求序号 0。");
            return None;
        }
        Some(epoch) => (epoch, false),
        None => (
            state
                .foreground_request_epoch
                .fetch_add(1, Ordering::SeqCst)
                + 1,
            true,
        ),
    };
    let current_epoch = state.foreground_request_epoch.load(Ordering::SeqCst);
    if epoch < current_epoch {
        log::info!(
            "[生命周期] 忽略过期前后台请求：请求序号={}，当前序号={}",
            epoch,
            current_epoch
        );
        return None;
    }
    if !is_implicit_request && epoch == current_epoch && current_epoch != 0 {
        if state.is_foreground.load(Ordering::SeqCst) != is_foreground {
            log::warn!("[生命周期] 同一请求序号对应冲突的前后台状态，拒绝处理。");
        }
        return None;
    }
    Some(epoch)
}

async fn apply_foreground_change(
    app: &AppHandle,
    state: &LifecycleState,
    was_foreground: bool,
    is_foreground: bool,
    _epoch: u64,
) {
    let transition_epoch = state.transition_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    log::info!(
        "[Lifecycle] 应用前后台状态切换：{} -> {}",
        was_foreground,
        is_foreground
    );
    crate::vcp_modules::infra::vcp_log_service::handle_foreground_state_change(app, is_foreground)
        .await;
    emit_lifecycle_event(app, is_foreground);
    if is_foreground {
        return_foreground(app, state).await;
    } else {
        enter_background(app, state, transition_epoch).await;
    }
}

fn emit_lifecycle_event(app: &AppHandle, is_foreground: bool) {
    let _ = app.emit(
        "vcp-lifecycle-changed",
        serde_json::json!({
            "state": if is_foreground { "resume" } else { "pause" }
        }),
    );
}

async fn enter_background(app: &AppHandle, state: &LifecycleState, epoch: u64) {
    cancel_linger_tasks(state).await;
    state
        .linger
        .is_log_disconnected
        .store(false, Ordering::SeqCst);
    state
        .linger
        .is_dist_disconnected
        .store(false, Ordering::SeqCst);
    let _ = tauri_plugin_vcp_mobile::stream::acquire_foreground_inner(
        app,
        "vcp_log",
        10,
        "VCP Log Linger",
        false,
    );
    start_log_linger(app, state, epoch).await;
    start_distributed_linger(app, state, epoch).await;
}

async fn cancel_linger_tasks(state: &LifecycleState) {
    let mut log_cancel = state.linger.log_cancel.lock().await;
    if let Some(token) = log_cancel.take() {
        token.cancel();
    }
    drop(log_cancel);
    let mut dist_cancel = state.linger.dist_cancel.lock().await;
    if let Some(token) = dist_cancel.take() {
        token.cancel();
    }
}

async fn start_log_linger(app: &AppHandle, state: &LifecycleState, epoch: u64) {
    let token = tokio_util::sync::CancellationToken::new();
    *state.linger.log_cancel.lock().await = Some(token.clone());
    let app = app.clone();
    let foreground = state.is_foreground.clone();
    let transition_epoch = state.transition_epoch.clone();
    let transition = state.transition.clone();
    let linger = state.linger.clone();
    crate::vcp_modules::infra::utils::spawn_linger_task(
        Duration::from_secs(600),
        token,
        move || async move {
            let _transition = transition.lock().await;
            if foreground.load(Ordering::SeqCst) || transition_epoch.load(Ordering::SeqCst) != epoch
            {
                return;
            }
            log::info!("[Lifecycle] 后台 10 分钟到期，断开 VCPLog/Info。");
            let _ =
                crate::vcp_modules::infra::vcp_log_service::disconnect_log_connections(&app).await;
            if !foreground.load(Ordering::SeqCst)
                && transition_epoch.load(Ordering::SeqCst) == epoch
            {
                linger.is_log_disconnected.store(true, Ordering::SeqCst);
                let _ = tauri_plugin_vcp_mobile::stream::release_foreground_inner(&app, "vcp_log");
            }
        },
    );
}

async fn start_distributed_linger(app: &AppHandle, state: &LifecycleState, epoch: u64) {
    if !ensure_background_distributed_config(app).await {
        return;
    }
    log::info!("[Lifecycle] 分布式已启用，安排 5 分钟后台保活任务。");
    let token = tokio_util::sync::CancellationToken::new();
    *state.linger.dist_cancel.lock().await = Some(token.clone());
    let app = app.clone();
    let foreground = state.is_foreground.clone();
    let transition_epoch = state.transition_epoch.clone();
    let transition = state.transition.clone();
    let linger = state.linger.clone();
    crate::vcp_modules::infra::utils::spawn_linger_task(
        Duration::from_secs(300),
        token,
        move || async move {
            stop_distributed_after_linger(
                &app,
                &transition,
                &foreground,
                &transition_epoch,
                &linger,
                epoch,
            )
            .await;
        },
    );
}

async fn ensure_background_distributed_config(app: &AppHandle) -> bool {
    let settings_state = match app.try_state::<SettingsState>() {
        Some(settings_state) => settings_state,
        None => {
            crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
                app,
                "后台保活时 SettingsState 未注册",
            )
            .await;
            return false;
        }
    };
    let settings = match read_settings(app.clone(), settings_state).await {
        Ok(settings) => settings,
        Err(error) => {
            crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
                app,
                format!("后台保活时读取设置失败：{error}"),
            )
            .await;
            return false;
        }
    };
    if !settings.distributed_enabled {
        crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
            app,
            "后台保活时权威设置已禁用分布式",
        )
        .await;
        return false;
    }
    if let Err(error) =
        crate::vcp_modules::infra::lifecycle_manager::validate_distributed_connection_config(
            &settings,
            "后台保活",
        )
    {
        crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(app, error)
            .await;
        return false;
    }
    true
}

async fn stop_distributed_after_linger(
    app: &AppHandle,
    transition: &std::sync::Arc<tokio::sync::Mutex<()>>,
    foreground: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    transition_epoch: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    linger: &LingerController,
    epoch: u64,
) {
    let _transition = transition.lock().await;
    if foreground.load(Ordering::SeqCst) || transition_epoch.load(Ordering::SeqCst) != epoch {
        return;
    }
    log::info!("[Lifecycle] 后台 5 分钟到期，停止分布式客户端。");
    if let Some(dist_state) = app.try_state::<crate::distributed::DistributedState>() {
        let client = dist_state.client.read().await;
        client.stop(app).await;
    }
    if !foreground.load(Ordering::SeqCst) && transition_epoch.load(Ordering::SeqCst) == epoch {
        linger.is_dist_disconnected.store(true, Ordering::SeqCst);
    }
}

async fn return_foreground(app: &AppHandle, state: &LifecycleState) {
    cancel_linger_tasks(state).await;
    let _ = tauri_plugin_vcp_mobile::stream::release_foreground_inner(app, "vcp_log");
    let Some(settings) = load_foreground_settings(app).await else {
        return;
    };
    let was_log_disconnected = state
        .linger
        .is_log_disconnected
        .swap(false, Ordering::SeqCst);
    let was_dist_disconnected = state
        .linger
        .is_dist_disconnected
        .swap(false, Ordering::SeqCst);
    restore_log_connections(app, &settings, was_log_disconnected).await;
    reconcile_foreground_distributed(app, &settings, was_dist_disconnected).await;
}

async fn load_foreground_settings(
    app: &AppHandle,
) -> Option<crate::vcp_modules::settings_manager::Settings> {
    let settings_state = match app.try_state::<SettingsState>() {
        Some(settings_state) => settings_state,
        None => {
            crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
                app,
                "回到前台时 SettingsState 未注册",
            )
            .await;
            return None;
        }
    };
    let settings = match read_settings(app.clone(), settings_state).await {
        Ok(settings) => settings,
        Err(error) => {
            crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
                app,
                format!("回到前台时读取设置失败：{error}"),
            )
            .await;
            return None;
        }
    };
    Some(settings)
}

async fn reconcile_foreground_distributed(
    app: &AppHandle,
    settings: &crate::vcp_modules::settings_manager::Settings,
    was_disconnected: bool,
) {
    if !settings.distributed_enabled {
        crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(
            app,
            "回到前台时权威设置已禁用分布式",
        )
        .await;
        return;
    }
    if let Err(error) =
        crate::vcp_modules::infra::lifecycle_manager::validate_distributed_connection_config(
            settings,
            "回到前台",
        )
    {
        crate::vcp_modules::infra::lifecycle_manager::stop_distributed_with_reason(app, error)
            .await;
        return;
    }
    log::info!(
        "[Lifecycle] 回到前台，调和分布式节点；此前冷断开：{}。",
        was_disconnected
    );
    crate::vcp_modules::infra::lifecycle_reconciler::reconcile_distributed_node(app, true, false)
        .await;
}

async fn restore_log_connections(
    app: &AppHandle,
    settings: &crate::vcp_modules::settings_manager::Settings,
    was_disconnected: bool,
) {
    if should_reconnect_log_connections(settings, was_disconnected) {
        log::info!("[Lifecycle] 回到前台，重连 VCPLog/Info。");
        let _ = crate::vcp_modules::infra::vcp_log_service::reconnect_log_connections(
            app,
            settings.vcp_log_url.clone(),
            settings.vcp_log_key.clone(),
        )
        .await;
    } else if !was_disconnected {
        crate::vcp_modules::infra::vcp_log_service::flush_background_logs(app);
    }
}

fn should_reconnect_log_connections(
    settings: &crate::vcp_modules::settings_manager::Settings,
    was_disconnected: bool,
) -> bool {
    was_disconnected
        && !settings.vcp_log_url.trim().is_empty()
        && !settings.vcp_log_key.trim().is_empty()
}

#[cfg(test)]
#[path = "lifecycle_transition_tests.rs"]
mod tests;
