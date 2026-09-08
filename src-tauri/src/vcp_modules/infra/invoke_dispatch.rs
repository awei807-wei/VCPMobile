use std::sync::Arc;

use tauri::{
    ipc::{Invoke, InvokeHandler},
    Runtime,
};

use super::invoke_guard;

type BeforeDispatchGate = Arc<dyn Fn() + Send + Sync + 'static>;

/// 创建应用级 IPC 外壳，把命令分派移入 Tokio worker。
///
/// 生成的命令处理器可共享；每次 invoke 都有独立的 worker 闭包。
/// 在闭包实际运行前保持 `Invoke` 完整，可把命令匹配、参数反序列化和
/// 命令 future 构造全部移出 Android JavaBridge 调用栈。
/// 发布构建保持 `panic = "abort"`；本外壳只保证非 panic 路径的 resolver
/// exactly-once，不对 panic 后的 Promise settle 作任何承诺。
pub fn central_invoke_handler<R: Runtime>(
    commands: Arc<InvokeHandler<R>>,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    central_invoke_handler_with_gate(commands, None)
}

fn central_invoke_handler_with_gate<R: Runtime>(
    commands: Arc<InvokeHandler<R>>,
    before_dispatch_gate: Option<BeforeDispatchGate>,
) -> impl Fn(Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        // JavaBridge 入口只读取到达时的轻量门禁快照；其余分派必须留在 worker。
        let reject_before_db_ready = invoke_guard::should_reject_before_db_ready(&invoke);
        let commands = Arc::clone(&commands);
        let before_dispatch_gate = before_dispatch_gate.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(gate) = before_dispatch_gate {
                gate();
            }
            let command = invoke.message.command().to_string();
            // 在生成处理器消费 `Invoke` 前保留 resolver；未知命令会返回 false，
            // 此时由这里显式拒绝，避免原 resolver 被静默丢弃。
            let resolver = invoke.resolver.clone();

            if reject_before_db_ready {
                invoke_guard::reject_core_not_ready(invoke);
            } else if !commands(invoke) {
                resolver.reject(unknown_command_error(&command));
            }
        });
        true
    }
}

fn unknown_command_error(command: &str) -> String {
    format!("Command {command} not found")
}

#[cfg(test)]
mod tests {
    use super::unknown_command_error;

    #[test]
    fn unknown_command_rejection_preserves_tauri_error_shape() {
        assert_eq!(
            unknown_command_error("missing_command"),
            "Command missing_command not found"
        );
    }
}

#[cfg(test)]
#[path = "invoke_dispatch_tests.rs"]
mod invoke_dispatch_tests;
