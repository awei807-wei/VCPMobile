use std::sync::Arc;

use tauri::{
    ipc::{Invoke, InvokeHandler},
    Wry,
};

use super::invoke_guard;

/// Creates the app-level IPC shell that moves command dispatch to a Tokio worker.
///
/// The generated command handler is shared because every incoming invoke gets its
/// own worker closure. Keeping the `Invoke` intact until that closure runs moves
/// command matching, argument deserialization, and command-future construction
/// off the Android JavaBridge stack.
pub fn central_invoke_handler(
    commands: Arc<InvokeHandler<Wry>>,
) -> impl Fn(Invoke<Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let commands = Arc::clone(&commands);
        tauri::async_runtime::spawn(async move {
            let command = invoke.message.command().to_string();
            // Keep a resolver clone before the generated handler consumes `Invoke`.
            // Unknown commands return false and drop the original resolver.
            let resolver = invoke.resolver.clone();

            if invoke_guard::should_reject_before_db_ready(&invoke) {
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
