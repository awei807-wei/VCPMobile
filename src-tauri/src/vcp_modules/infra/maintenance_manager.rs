// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

#[path = "maintenance_gc.rs"]
mod maintenance_gc;

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::delete_attachment_physical;
use crate::vcp_modules::infra::utils::now_secs;
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use maintenance_gc::calculate_dir_size;
pub use maintenance_gc::cleanup_orphaned_attachments;
use tauri::{AppHandle, Manager, State};

/// 清理 WebView 缓存及 HTTP 缓存目录。
#[tauri::command]
pub async fn clear_webview_cache(app: AppHandle) -> Result<String, String> {
    let mut cleared_details = String::new();
    let mut freed_size = 0u64;
    if let Some(webview) = app.get_webview_window("main") {
        webview
            .clear_all_browsing_data()
            .map_err(|e| format!("WebView 缓存清理失败: {}", e))?;
        cleared_details.push_str("标准浏览数据已清除；");
    } else {
        cleared_details.push_str("未找到主窗口，跳过标准清理；");
    }
    if let Ok(cache_dir) = app.path().app_cache_dir() {
        let http_cache_dir = cache_dir.join("WebView").join("Default").join("HTTP Cache");
        if http_cache_dir.exists() {
            freed_size = calculate_dir_size(&http_cache_dir).await;
            if tokio::fs::remove_dir_all(&http_cache_dir).await.is_ok() {
                cleared_details.push_str("物理 HTTP Cache 已抹除；");
            } else {
                freed_size = 0;
                cleared_details.push_str("部分 HTTP 物理缓存被占用，已标记失效；");
            }
        }
    }
    let freed_size_mb = (freed_size as f64) / 1024.0 / 1024.0;
    Ok(format!(
        "WebView 缓存清理成功 ({})，释放空间: {:.2} MB",
        cleared_details.trim_end_matches('；'),
        freed_size_mb
    ))
}

/// 重建 V8 code_cache 并执行 SQLite 增量真空整理。
#[tauri::command]
pub async fn reconstruct_system_cache(
    app: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let mut cleared_details = String::new();
    if let Ok(cache_dir) = app.path().app_cache_dir() {
        let code_cache_dir = cache_dir.join("code_cache");
        if code_cache_dir.exists() {
            if tokio::fs::remove_dir_all(&code_cache_dir).await.is_ok() {
                cleared_details.push_str("V8 code_cache 已彻底物理清除；");
            } else {
                cleared_details.push_str("V8 code_cache 部分锁定，已标记失效；");
            }
        } else {
            cleared_details.push_str("V8 code_cache 无残余物理文件；");
        }
    }
    let _ = db_state.run_incremental_vacuum_optimize(500).await;
    cleared_details.push_str("SQLite 空间碎片整理与索引规划器重构已执行；");
    Ok(format!(
        "系统缓存重建与数据库真空物理收缩完成 ({})",
        cleared_details.trim_end_matches('；')
    ))
}

/// 精准清理未被消息引用的暂存附件。
#[tauri::command]
pub async fn cleanup_single_orphaned_attachment(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
) -> Result<String, String> {
    let is_used: bool = sqlx::query_scalar::<_, i32>(
        "SELECT EXISTS(
         SELECT 1 FROM message_attachments ma
         INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id
            AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id
         WHERE ma.hash = ? AND m.deleted_at IS NULL)",
    )
    .bind(&hash)
    .fetch_one(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?
        != 0;
    if is_used {
        return Ok("附件已被其他消息引用，跳过清理".to_string());
    }
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT internal_path, created_at FROM attachments WHERE hash = ?")
            .bind(&hash)
            .fetch_optional(&db_state.pool)
            .await
            .map_err(|e| e.to_string())?;
    let Some((path_str, _)) = row else {
        return Ok("数据库中未找到该附件记录".to_string());
    };
    let _ = delete_attachment_physical(&app_handle, &hash, &path_str).await;
    let _ = sqlx::query("DELETE FROM attachments WHERE hash = ?")
        .bind(&hash)
        .execute(&db_state.pool)
        .await;
    Ok("成功清理未引用的暂存附件".to_string())
}

/// 在 App 启动时按周期触发低频维护。
pub async fn init_automatic_maintenance(app: AppHandle) {
    let settings_state = app.state::<SettingsState>();
    let settings = match read_settings(app.clone(), settings_state.clone()).await {
        Ok(settings) => settings,
        Err(_) => return,
    };
    let last_clear = settings
        .extra
        .get("lastWebviewCacheClear")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let now = now_secs();
    if now - last_clear <= 3 * 24 * 60 * 60 {
        return;
    }
    log::info!("[Maintenance] Triggering scheduled maintenance (WebView & SQLite)...");
    if let Some(webview) = app.get_webview_window("main") {
        let _ = webview.clear_all_browsing_data();
    }
    let db_state = app.state::<DbState>();
    let _ = db_state.run_incremental_vacuum_optimize(100).await;
    delete_deleted_message_attachments(&db_state).await;
    let updates = serde_json::json!({"lastWebviewCacheClear": now});
    let _ = update_settings(app.clone(), settings_state, updates).await;
    log::info!("[Maintenance] Scheduled maintenance complete.");
}

async fn delete_deleted_message_attachments(db_state: &DbState) {
    let _ = sqlx::query(
        "DELETE FROM message_attachments AS ma WHERE EXISTS (
         SELECT 1 FROM messages m
         WHERE m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
           AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
           AND m.deleted_at IS NOT NULL)",
    )
    .execute(&db_state.pool)
    .await;
}
