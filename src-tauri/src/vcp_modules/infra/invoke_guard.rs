use tauri::{ipc::Invoke, Manager, Runtime};

use crate::vcp_modules::db_manager::{DbState, CORE_NOT_READY_ERROR};

/// 仅允许启动诊断与核心状态查询在数据库注册前执行。
///
/// 插件命令由各插件自行管理状态，不受主应用数据库门禁约束。
pub fn can_run_before_db_ready(command: &str) -> bool {
    command.starts_with("plugin:")
        || matches!(
            command,
            "get_system_snapshot"
                | "get_core_status"
                | "get_last_error"
                | "confirm_frontend_boot"
                | "record_frontend_diagnostic"
                | "export_runtime_diagnostics"
        )
}

/// 判断当前 IPC 是否应在数据库尚未注册时被拒绝。
pub fn should_reject_before_db_ready<R: Runtime>(invoke: &Invoke<R>) -> bool {
    let command = invoke.message.command();
    !can_run_before_db_ready(command)
        && invoke
            .message
            .webview_ref()
            .app_handle()
            .try_state::<DbState>()
            .is_none()
}

/// 以可识别的 `CORE_NOT_READY` 错误拒绝过早到达的 IPC。
pub fn reject_core_not_ready<R: Runtime>(invoke: Invoke<R>) -> bool {
    let command = invoke.message.command().to_string();
    log::warn!(
        "[InvokeGuard] Rejected command before DbState registration: {}",
        command
    );
    invoke
        .resolver
        .reject(format!("{} command={}", CORE_NOT_READY_ERROR, command));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_diagnostics_and_plugin_commands_remain_available() {
        for command in [
            "plugin:vcp-mobile|check_all_permissions",
            "get_system_snapshot",
            "get_core_status",
            "get_last_error",
            "confirm_frontend_boot",
            "record_frontend_diagnostic",
            "export_runtime_diagnostics",
        ] {
            assert!(
                can_run_before_db_ready(command),
                "command should remain available during bootstrap: {command}"
            );
        }
    }

    #[test]
    fn database_commands_fail_closed_until_db_state_exists() {
        for command in [
            "load_chat_history",
            "get_avatar",
            "read_settings",
            "get_assistants_snapshot",
            "get_topics_streamed",
        ] {
            assert!(
                !can_run_before_db_ready(command),
                "database command must be gated during bootstrap: {command}"
            );
        }
    }
}
