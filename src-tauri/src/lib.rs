#![allow(dead_code)]

mod bootstrap;
mod distributed;
mod vcp_modules;

use vcp_modules::agent_chat_application_service::{
    handle_agent_chat_message, handle_assistant_chat_stream, is_assistant_chat_active,
};
use vcp_modules::agent_service::{
    create_agent, delete_agent, get_agents, get_assistants_snapshot, read_agent_config,
    save_agent_config, update_agent_config,
};
use vcp_modules::avatar_service::{
    batch_get_avatars, get_avatar, save_avatar_data, store_dominant_color,
};
use vcp_modules::chat_manager::{
    append_single_message, delete_messages, edit_message_and_truncate_history, load_chat_history,
    load_chat_history_around, load_chat_history_streamed, patch_single_message,
    truncate_history_after_timestamp,
};
use vcp_modules::context_injection::{
    delete_tarven_rule, get_tarven_rules, preview_tarven_injection, reorder_rules,
    save_tarven_rule, toggle_rule_enabled,
};
use vcp_modules::db_manager::{get_fts_index_status, rebuild_messages_fts, search_messages_fts};
use vcp_modules::emoticon_manager::{
    fix_emoticon_url, get_emoticon_library, regenerate_emoticon_library,
};
use vcp_modules::file_manager::{
    check_attachment_support, get_attachment_real_path, open_file, register_local_file, store_file,
};
use vcp_modules::frontend_update_manager::{
    apply_frontend_update, check_for_frontend_update, clear_frontend_updates,
    confirm_frontend_boot, download_frontend_update, get_active_frontend_version,
};
use vcp_modules::group_chat_application_service::handle_group_chat_message;
use vcp_modules::group_service::{
    create_group, delete_group, get_groups, read_group_config, save_group_config,
    update_group_config,
};
use vcp_modules::high_speed_channel::prepare_vcp_upload;
use vcp_modules::infra::lifecycle_controller::set_app_foreground_state;
use vcp_modules::lifecycle_manager::{
    get_core_status, get_last_error, get_system_snapshot, reconcile_distributed_node_cmd,
    reconcile_local_server_cmd,
};
use vcp_modules::maintenance_manager::{
    cleanup_orphaned_attachments, cleanup_single_orphaned_attachment, clear_webview_cache,
    reconstruct_system_cache,
};
use vcp_modules::message_repository::{process_message_content, rebuild_all_pre_renders};
use vcp_modules::message_service::delete_message_attachment;
use vcp_modules::message_service::{fetch_raw_message_content, re_render_message};
use vcp_modules::model_manager::{
    get_cached_models, get_favorite_models, get_hot_models, invalidate_model_cache,
    record_model_usage, refresh_models, start_batch_model_test, stop_all_model_tests,
    test_model_connectivity, toggle_favorite_model,
};
use vcp_modules::runtime_diagnostics::{export_runtime_diagnostics, record_frontend_diagnostic};
use vcp_modules::settings_manager::{
    begin_connection_profile_switch, end_connection_profile_switch,
    is_connection_profile_switching_command, read_settings, set_theme, update_settings,
    write_settings,
};
#[cfg(debug_assertions)]
use vcp_modules::sync::wire14_debug::{
    debug_get_wire14_scale_topic_hashes, debug_inject_invalid_attachment_for_wire14,
};
use vcp_modules::sync_service::{
    clear_old_sync_logs, get_sync_session_log_path, get_sync_status, is_sync_active,
    list_sync_log_files, read_sync_log_file, start_manual_sync, stop_sync,
};
use vcp_modules::topic_service::{
    archive_assistant_chat, create_topic, delete_topic, get_owner_unread_count, get_topics,
    get_topics_streamed, get_unread_counts, increment_topic_unread_count,
    regenerate_topic_response, set_topic_unread, summarize_topic, toggle_topic_lock,
    update_topic_title,
};
use vcp_modules::update_manager::{check_for_update, download_update, install_update};
use vcp_modules::vcp_client::{
    get_active_generations, interruptGroupTurn, interruptRequest, recover_active_generation,
    resume_stream, sendToVCP, test_vcp_connection,
};
use vcp_modules::vcp_info_service::{
    clear_vcp_info, get_vcp_info_connection_status, get_vcp_info_metadata_list,
    get_vcp_info_payload, init_vcp_info_connection,
};
use vcp_modules::vcp_log_service::{
    init_vcp_log_connection, send_vcp_log_message, set_vcp_log_heartbeat,
};

macro_rules! build_invoke_handler {
    () => {{
        let handler: std::sync::Arc<tauri::ipc::InvokeHandler<tauri::Wry>> =
            std::sync::Arc::new(tauri::generate_handler![
                sendToVCP,
                get_tarven_rules,
                save_tarven_rule,
                delete_tarven_rule,
                toggle_rule_enabled,
                reorder_rules,
                preview_tarven_injection,
                interruptRequest,
                interruptGroupTurn,
                test_vcp_connection,
                get_active_generations,
                recover_active_generation,
                resume_stream,
                handle_agent_chat_message,
                handle_assistant_chat_stream,
                is_assistant_chat_active,
                load_chat_history,
                load_chat_history_around,
                load_chat_history_streamed,
                search_messages_fts,
                get_fts_index_status,
                rebuild_messages_fts,
                append_single_message,
                edit_message_and_truncate_history,
                patch_single_message,
                delete_messages,
                delete_message_attachment,
                truncate_history_after_timestamp,
                process_message_content,
                rebuild_all_pre_renders,
                get_topics,
                get_topics_streamed,
                get_unread_counts,
                get_owner_unread_count,
                increment_topic_unread_count,
                get_groups,
                read_group_config,
                create_topic,
                delete_topic,
                update_topic_title,
                toggle_topic_lock,
                set_topic_unread,
                regenerate_topic_response,
                get_agents,
                get_assistants_snapshot,
                read_agent_config,
                save_agent_config,
                update_agent_config,
                save_avatar_data,
                get_avatar,
                batch_get_avatars,
                store_dominant_color,
                read_settings,
                write_settings,
                update_settings,
                begin_connection_profile_switch,
                end_connection_profile_switch,
                is_connection_profile_switching_command,
                handle_group_chat_message,
                create_agent,
                create_group,
                save_group_config,
                update_group_config,
                delete_group,
                delete_agent,
                set_theme,
                store_file,
                check_attachment_support,
                register_local_file,
                prepare_vcp_upload,
                fetch_raw_message_content,
                re_render_message,
                get_attachment_real_path,
                open_file,
                clear_webview_cache,
                reconstruct_system_cache,
                cleanup_orphaned_attachments,
                cleanup_single_orphaned_attachment,
                get_cached_models,
                refresh_models,
                invalidate_model_cache,
                get_hot_models,
                get_favorite_models,
                toggle_favorite_model,
                record_model_usage,
                test_model_connectivity,
                start_batch_model_test,
                stop_all_model_tests,
                summarize_topic,
                init_vcp_log_connection,
                send_vcp_log_message,
                set_vcp_log_heartbeat,
                init_vcp_info_connection,
                get_vcp_info_connection_status,
                get_vcp_info_metadata_list,
                get_vcp_info_payload,
                clear_vcp_info,
                get_system_snapshot,
                get_emoticon_library,
                regenerate_emoticon_library,
                fix_emoticon_url,
                get_core_status,
                get_last_error,
                set_app_foreground_state,
                get_sync_status,
                is_sync_active,
                start_manual_sync,
                stop_sync,
                get_sync_session_log_path,
                list_sync_log_files,
                read_sync_log_file,
                clear_old_sync_logs,
                archive_assistant_chat,
                reconcile_local_server_cmd,
                reconcile_distributed_node_cmd,
                distributed::get_distributed_status,
                distributed::get_registered_tools_metadata,
                distributed::update_enabled_tools,
                distributed::get_distributed_tool_config_status,
                distributed::reset_distributed_tools_disabled,
                distributed::execute_distributed_tool,
                distributed::reconnect_distributed_client,
                check_for_update,
                download_update,
                install_update,
                check_for_frontend_update,
                download_frontend_update,
                apply_frontend_update,
                get_active_frontend_version,
                clear_frontend_updates,
                confirm_frontend_boot,
                record_frontend_diagnostic,
                export_runtime_diagnostics,
                #[cfg(debug_assertions)]
                debug_get_wire14_scale_topic_hashes,
                #[cfg(debug_assertions)]
                debug_inject_invalid_attachment_for_wire14,
            ]);
        vcp_modules::infra::invoke_dispatch::central_invoke_handler(handler)
    }};
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    bootstrap::run(build_invoke_handler!());
}

#[cfg(test)]
#[test]
fn handler_registration_smoke() {
    let _handler = build_invoke_handler!();
}
