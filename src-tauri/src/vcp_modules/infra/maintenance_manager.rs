// maintenance_manager.rs - 负责系统维护、垃圾回收与缓存清理的核心模块
// 职责: 聚合所有低频但关键的系统维护任务，对齐前端 MaintenanceSection 领域。

use std::future::Future;

#[path = "maintenance_gc.rs"]
mod maintenance_gc;

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::infra::utils::now_secs;
use crate::vcp_modules::settings_manager::{read_settings, update_settings, SettingsState};
use maintenance_gc::calculate_dir_size;
pub use maintenance_gc::cleanup_orphaned_attachments;
pub(crate) use maintenance_gc::{
    clear_live_attachment_unlink_debts, clear_live_attachment_unlink_debts_rusqlite,
    managed_attachment_roots, ManagedAttachmentRoots,
};
use tauri::{AppHandle, Manager, State};

const AUTOMATIC_GC_PAGE_BUDGET: usize = 4;
const AUTOMATIC_GC_YIELD_DELAY: std::time::Duration = std::time::Duration::from_millis(10);
const AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS: usize = 32;

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
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(&hash) {
        return Err("附件哈希格式无效".to_string());
    }
    let reclaimed =
        maintenance_gc::reclaim_single_orphaned_attachment(&app_handle, &db_state.pool, &hash)
            .await
            .map_err(|error| format!("附件清理失败: {error}"))?;
    if reclaimed {
        Ok("成功清理未引用的暂存附件".to_string())
    } else {
        Ok("附件仍被引用、路径不受管或已延期清理".to_string())
    }
}

/// 在 App 启动时按周期触发低频维护。
pub async fn init_automatic_maintenance(app: AppHandle) {
    // GC 在 Core Ready 后按启动维护周期运行；附件注册与 GC 通过共享闸门协调，
    // 不依赖“上传前”时序。即使 WebView 三日维护尚未到期，附件 GC 也会按启动周期执行。
    let db_pool = app.try_state::<DbState>().map(|state| state.pool.clone());
    if let Some(pool) = db_pool {
        match maintenance_gc::reclaim_orphaned_attachments_with_budget(
            &app,
            &pool,
            AUTOMATIC_GC_PAGE_BUDGET,
        )
        .await
        {
            Ok(report) => {
                log_attachment_gc_report(&report);
                if report.has_more {
                    schedule_attachment_gc_continuation(app.clone());
                }
            }
            Err(error) => log::warn!("[Maintenance] Attachment GC deferred: {error}"),
        }
    }
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
    let updates = serde_json::json!({"lastWebviewCacheClear": now});
    let _ = update_settings(app.clone(), settings_state, updates).await;
    log::info!("[Maintenance] Scheduled maintenance complete.");
}

fn log_attachment_gc_report(report: &maintenance_gc::AttachmentGcReport) {
    let status = if report.has_more || report.deferred > 0 {
        "deferred"
    } else {
        "complete"
    };
    log::info!(
        "[Maintenance] Attachment GC {status}: retained={}, reclaimed={}, ghosts={}, deferred={}, has_more={}",
        report.retained,
        report.reclaimed,
        report.ghost_files,
        report.deferred,
        report.has_more
    );
}

fn schedule_attachment_gc_continuation(app: AppHandle) {
    let runner_app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_attachment_gc_continuation(
            AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS,
            move || {
                let app = runner_app.clone();
                async move {
                    let Some(pool) = app.try_state::<DbState>().map(|state| state.pool.clone())
                    else {
                        return Err("附件 GC continuation 缺少数据库状态".to_string());
                    };
                    maintenance_gc::reclaim_orphaned_attachments_with_budget(
                        &app,
                        &pool,
                        AUTOMATIC_GC_PAGE_BUDGET,
                    )
                    .await
                }
            },
            || async {
                tokio::task::yield_now().await;
                tokio::time::sleep(AUTOMATIC_GC_YIELD_DELAY).await;
            },
        )
        .await;
    });
}

async fn run_attachment_gc_continuation<Run, RunFuture, Delay, DelayFuture>(
    max_rounds: usize,
    mut run_page: Run,
    mut delay: Delay,
) where
    Run: FnMut() -> RunFuture + Send + 'static,
    RunFuture: Future<Output = Result<maintenance_gc::AttachmentGcReport, String>> + Send,
    Delay: FnMut() -> DelayFuture + Send + 'static,
    DelayFuture: Future<Output = ()> + Send,
{
    for round in 0..max_rounds {
        delay().await;
        let report = match run_page().await {
            Ok(report) => report,
            Err(error) => {
                log::warn!("[Maintenance] Attachment GC continuation deferred: {error}");
                break;
            }
        };
        log_attachment_gc_report(&report);
        if !report.has_more {
            return;
        }
        if round + 1 == max_rounds {
            log::warn!(
                "[Maintenance] Attachment GC continuation reached the {}-round safety limit",
                max_rounds
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        maintenance_gc::AttachmentGcReport, run_attachment_gc_continuation,
        AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn continuation_stops_after_exactly_32_rounds_when_work_never_exhausts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let delays = Arc::new(AtomicUsize::new(0));
        let runner_calls = calls.clone();
        let delay_calls = delays.clone();
        run_attachment_gc_continuation(
            AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS,
            move || {
                runner_calls.fetch_add(1, Ordering::SeqCst);
                async {
                    Ok(AttachmentGcReport {
                        has_more: true,
                        ..AttachmentGcReport::default()
                    })
                }
            },
            move || {
                delay_calls.fetch_add(1, Ordering::SeqCst);
                async {}
            },
        )
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS
        );
        assert_eq!(
            delays.load(Ordering::SeqCst),
            AUTOMATIC_GC_MAX_CONTINUATION_ROUNDS
        );
    }
}
