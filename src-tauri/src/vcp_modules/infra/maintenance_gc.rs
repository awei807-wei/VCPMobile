#[path = "maintenance_gc_batch.rs"]
mod batch;
#[path = "maintenance_gc_db.rs"]
mod db;
#[path = "maintenance_gc_io.rs"]
mod io;
#[path = "maintenance_gc_outbox.rs"]
mod outbox;
#[path = "maintenance_gc_paths.rs"]
mod paths;
#[path = "maintenance_gc_pipeline.rs"]
mod pipeline;
#[path = "maintenance_gc_reference.rs"]
mod reference;
#[path = "maintenance_gc_write.rs"]
mod write;

use tauri::{AppHandle, State};

use self::db::{read_gc_cursor, ATTACHMENT_GC_CURSOR_KEY, RELATION_GC_CURSOR_KEY};
#[cfg(test)]
use self::pipeline::commit_gc_batch_inner;
use self::pipeline::{build_gc_batch, build_single_batch, commit_gc_batch};
pub(crate) use self::write::{
    clear_live_attachment_unlink_debts, clear_live_attachment_unlink_debts_rusqlite,
};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    attachment_gc_gate, get_attachments_root_dir, get_multimodal_cache_dir, get_thumbnails_root_dir,
};
pub(super) use io::calculate_dir_size;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AttachmentGcReport {
    pub retained: usize,
    pub reclaimed: usize,
    pub ghost_files: usize,
    pub deferred: usize,
    pub has_more: bool,
    pub cursor: Option<String>,
}

pub(crate) use paths::ManagedAttachmentRoots;

/// 根据应用配置解析附件 GC 使用的真实 managed roots。
///
/// 生产写入口必须复用该路径来源，不能从数据库路径或目录 basename 猜测 root。
pub(crate) fn managed_attachment_roots<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<ManagedAttachmentRoots, String> {
    Ok(ManagedAttachmentRoots::new(
        get_attachments_root_dir(app_handle)?,
        get_thumbnails_root_dir(app_handle)?,
        get_multimodal_cache_dir(app_handle)?,
    ))
}

#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let report = reclaim_orphaned_attachments(&app_handle, &db_state.pool).await?;
    let total_deleted = report.reclaimed + report.ghost_files;
    let status = if report.has_more || report.deferred > 0 {
        "清理部分完成，仍有工作待处理"
    } else {
        "清理完成"
    };
    Ok(format!(
        "{status}：删除 {} 个孤立文件，回收 {} 条附件索引，保留 {} 个有效附件，延期 {} 条{}",
        total_deleted,
        report.reclaimed,
        report.retained,
        report.deferred,
        report
            .has_more
            .then_some("，仍有分页待处理")
            .unwrap_or_default()
    ))
}

/// 执行一页有界、幂等的附件回收；写闸门覆盖提交后的物理清理窗口。
pub async fn reclaim_orphaned_attachments<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
) -> Result<AttachmentGcReport, String> {
    let roots = managed_attachment_roots(app_handle)?;
    reclaim_orphaned_attachments_at_roots(pool, &roots).await
}

/// Consume at most `page_budget` GC pages and yield between pages. A caller
/// can schedule another bounded tick when the returned report still has work.
pub async fn reclaim_orphaned_attachments_with_budget<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    page_budget: usize,
) -> Result<AttachmentGcReport, String> {
    let roots = managed_attachment_roots(app_handle)?;
    reclaim_orphaned_attachments_at_roots_with_budget(pool, &roots, page_budget).await
}

async fn reclaim_orphaned_attachments_at_roots_with_budget(
    pool: &sqlx::SqlitePool,
    roots: &ManagedAttachmentRoots,
    page_budget: usize,
) -> Result<AttachmentGcReport, String> {
    let mut combined = AttachmentGcReport::default();
    let page_budget = page_budget.max(1);
    for page in 0..page_budget {
        let report = reclaim_orphaned_attachments_at_roots(pool, roots).await?;
        combined.retained += report.retained;
        combined.reclaimed += report.reclaimed;
        combined.ghost_files += report.ghost_files;
        combined.deferred += report.deferred;
        combined.has_more = report.has_more;
        combined.cursor = report.cursor;
        if !report.has_more || page + 1 == page_budget {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(combined)
}

pub async fn reclaim_single_orphaned_attachment<R: tauri::Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    hash: &str,
) -> Result<bool, String> {
    let roots = managed_attachment_roots(app_handle)?;
    reclaim_single_orphaned_attachment_at_roots(pool, &roots, hash).await
}

#[cfg(test)]
async fn reclaim_with_commit_failure_at_roots(
    pool: &sqlx::SqlitePool,
    roots: &ManagedAttachmentRoots,
) -> Result<AttachmentGcReport, String> {
    let _gate = attachment_gc_gate().write().await;
    let mut connection = pool
        .acquire()
        .await
        .map_err(|error| format!("获取附件 GC 数据库连接失败: {error}"))?;
    begin_gc_transaction(&mut connection).await?;
    let cursor = read_gc_cursor(&mut *connection, ATTACHMENT_GC_CURSOR_KEY).await?;
    let relation_cursor = read_gc_cursor(&mut *connection, RELATION_GC_CURSOR_KEY)
        .await?
        .parse::<i64>()
        .unwrap_or_default();
    let result = build_gc_batch(&mut connection, roots, &cursor, relation_cursor).await;
    commit_gc_batch_inner(&mut connection, roots, result, true).await
}

async fn reclaim_orphaned_attachments_at_roots(
    pool: &sqlx::SqlitePool,
    roots: &ManagedAttachmentRoots,
) -> Result<AttachmentGcReport, String> {
    let _gate = attachment_gc_gate().write().await;
    let mut connection = pool
        .acquire()
        .await
        .map_err(|error| format!("获取附件 GC 数据库连接失败: {error}"))?;
    begin_gc_transaction(&mut connection).await?;
    let cursor = read_gc_cursor(&mut *connection, ATTACHMENT_GC_CURSOR_KEY).await?;
    let relation_cursor = read_gc_cursor(&mut *connection, RELATION_GC_CURSOR_KEY)
        .await?
        .parse::<i64>()
        .unwrap_or_default();
    let result = build_gc_batch(&mut connection, roots, &cursor, relation_cursor).await;
    commit_gc_batch(&mut connection, roots, result).await
}

async fn reclaim_single_orphaned_attachment_at_roots(
    pool: &sqlx::SqlitePool,
    roots: &ManagedAttachmentRoots,
    hash: &str,
) -> Result<bool, String> {
    let _gate = attachment_gc_gate().write().await;
    let mut connection = pool
        .acquire()
        .await
        .map_err(|error| format!("获取附件 GC 数据库连接失败: {error}"))?;
    begin_gc_transaction(&mut connection).await?;
    let result = build_single_batch(&mut connection, roots, hash).await;
    match result {
        Ok(batch) => {
            sqlx::query("COMMIT")
                .execute(&mut *connection)
                .await
                .map_err(|error| format!("提交附件 CAS 事务失败: {error}"))?;
            let reclaimed = batch.report.reclaimed == 1;
            Ok(reclaimed)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn begin_gc_transaction(
    connection: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
) -> Result<(), String> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut **connection)
        .await
        .map(|_| ())
        .map_err(|error| format!("开启附件 GC CAS 事务失败: {error}"))
}

#[cfg(test)]
#[path = "maintenance_gc_tests.rs"]
mod tests;
