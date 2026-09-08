use crate::{distributed, vcp_modules};
use tauri::{Listener, Manager};
use vcp_modules::context_sanitizer::ContextSanitizer;
use vcp_modules::infra::lifecycle_state::LifecycleState;
use vcp_modules::lifecycle_manager::{bootstrap, recover_distributed_node_after_network_restore};
use vcp_modules::maintenance_manager::init_automatic_maintenance;
use vcp_modules::vcp_client::{ActiveRequests, CancelledGroupTurns};

pub(crate) fn run(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) {
    let mut context = tauri::generate_context!();
    configure_context_assets(&mut context);
    configure_builder(handler)
        .run(context)
        .expect("error while running tauri application");
}

fn configure_context_assets(context: &mut tauri::Context<tauri::Wry>) {
    let identifier = context.config().identifier.clone();
    #[cfg(target_os = "android")]
    let active_version_path = format!(
        "/data/data/{}/files/frontend_updates/active_version",
        identifier
    );
    #[cfg(not(target_os = "android"))]
    let active_version_path = String::new();

    let update_dir = if cfg!(target_os = "android") {
        if let Ok(version) = std::fs::read_to_string(&active_version_path) {
            let version = version.trim();
            if version.is_empty() {
                std::path::PathBuf::new()
            } else {
                std::path::PathBuf::from(format!(
                    "/data/data/{}/files/frontend_updates/{}",
                    identifier, version
                ))
            }
        } else {
            std::path::PathBuf::new()
        }
    } else {
        std::path::PathBuf::new()
    };

    let embedded = context.set_assets(Box::new(vcp_modules::ota_assets::EmptyAssets));
    let ota_assets = vcp_modules::ota_assets::OtaAssets::new(embedded, update_dir);
    context.set_assets(Box::new(ota_assets));
}

fn configure_builder(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .setup(setup_app)
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets({
                    #[cfg(any(debug_assertions, not(mobile)))]
                    {
                        vec![
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                                file_name: None,
                            }),
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                        ]
                    }
                    #[cfg(not(any(debug_assertions, not(mobile))))]
                    {
                        vec![
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                            tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                                file_name: None,
                            }),
                        ]
                    }
                })
                .level(log::LevelFilter::Info)
                .filter(|metadata| {
                    let target = metadata.target();
                    !target.contains("pointer")
                        && !target.contains("touch")
                        && !target.contains("gesture")
                        && !target.contains("wry::event_loop")
                        && !target.contains("tao::window")
                        && !target.contains("wry::webview")
                        && !target.contains("DynamicFramerate")
                        && !target.contains("PowerHalMgrImpl")
                        && !target.contains("AnimationSpeedAware")
                        && !target.contains("InputEventInfo")
                })
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_vcp_mobile::init())
        .invoke_handler(handler)
}

fn setup_app(app: &mut tauri::App<tauri::Wry>) -> Result<(), Box<dyn std::error::Error>> {
    vcp_modules::runtime_diagnostics::install_panic_hook(app.handle().clone());
    manage_initial_state(app);
    let handle = app.handle().clone();
    vcp_modules::frontend_update_manager::clear_on_apk_upgrade(&handle);
    vcp_modules::frontend_update_manager::rollback_if_needed(&handle);
    vcp_modules::frontend_update_manager::safe_cleanup_old_versions(&handle);
    vcp_modules::file_manager::clear_upload_cache(&handle);
    spawn_bootstrap(handle.clone());
    install_network_listener(app);
    Ok(())
}

fn manage_initial_state(app: &mut tauri::App<tauri::Wry>) {
    app.manage(app.handle().clone());
    app.manage(LifecycleState::new());
    app.manage(ActiveRequests::default());
    app.manage(vcp_modules::agent_chat_application_service::AssistantChatActivityState::default());
    app.manage(CancelledGroupTurns::default());
    app.manage(ContextSanitizer::default());
    app.manage(distributed::DistributedState::new());
    let owner_locks = vcp_modules::owner_lock::OwnerLockRegistry::new();
    app.manage(owner_locks.clone());
    app.manage(vcp_modules::agent_service::AgentConfigState::with_owner_locks(owner_locks.clone()));
    app.manage(vcp_modules::group_service::GroupManagerState::with_owner_locks(owner_locks));
    app.manage(vcp_modules::settings_manager::SettingsState::new());
    app.manage(vcp_modules::settings_manager::ConnectionProfileSwitchState::default());
    app.manage(vcp_modules::model_manager::ModelManagerState::new());
    app.manage(vcp_modules::emoticon_manager::EmoticonManagerState::default());
}

fn spawn_bootstrap(handle: tauri::AppHandle<tauri::Wry>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = bootstrap(&handle).await {
            log::error!("[VCPCore] Bootstrap failed: {}", error);
            return;
        }
        let maintenance_handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            init_automatic_maintenance(maintenance_handle).await;
        });
    });
}

fn install_network_listener(app: &mut tauri::App<tauri::Wry>) {
    let handle = app.handle().clone();
    app.listen_any("vcp-network-status-changed", move |event| {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        if !payload
            .get("connected")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return;
        }
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            recover_distributed_node_after_network_restore(&handle).await;
        });
    });
}
