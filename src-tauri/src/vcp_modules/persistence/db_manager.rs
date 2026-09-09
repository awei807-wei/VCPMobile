use sqlx::{Pool, Sqlite};
use tauri::{AppHandle, Manager, Runtime, State};

use super::global_search::{
    index_integrity::{FtsIndexStatus, FtsRebuildResult},
    FtsSearchFilter, FtsSearchPage,
};

#[allow(unused_imports)]
pub use super::global_search::FtsSearchResult;

pub const CORE_NOT_READY_ERROR: &str = "CORE_NOT_READY: 数据库尚未初始化，请稍后重试。";

pub struct DbState {
    pub pool: Pool<Sqlite>,
    pub path: std::path::PathBuf,
}

/// 获取已完成初始化的数据库状态；启动竞态期间返回可恢复错误而不是触发 panic。
pub fn require_db_state<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<State<'_, DbState>, String> {
    app_handle
        .try_state::<DbState>()
        .ok_or_else(|| CORE_NOT_READY_ERROR.to_string())
}

impl DbState {
    /// 执行 SQLite 物理页面碎片分批回收与查询规划器索引优化
    pub async fn run_incremental_vacuum_optimize(
        &self,
        pages_to_vacuum: i32,
    ) -> Result<(), sqlx::Error> {
        // 1. 分批页整理碎片，防堵大面积 I/O 阻塞
        sqlx::query(&format!("PRAGMA incremental_vacuum({})", pages_to_vacuum))
            .execute(&self.pool)
            .await?;
        // 2. 重构索引规划器
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        Ok(())
    }
}

pub async fn init_db(app_handle: &AppHandle) -> Result<(Pool<Sqlite>, std::path::PathBuf), String> {
    super::database_lifecycle::init_db(app_handle).await
}

/// 全局搜索的稳定 Tauri 注册入口；查询和摘要逻辑位于独立职责模块。
#[tauri::command]
pub async fn search_messages_fts(
    db_state: State<'_, DbState>,
    filter: FtsSearchFilter,
) -> Result<FtsSearchPage, String> {
    super::global_search::search_messages(&db_state.pool, filter).await
}

#[tauri::command]
pub async fn get_fts_index_status(db_state: State<'_, DbState>) -> Result<FtsIndexStatus, String> {
    super::global_search::index_integrity::get_fts_index_status(&db_state.pool).await
}

#[tauri::command]
pub async fn rebuild_messages_fts(
    db_state: State<'_, DbState>,
) -> Result<FtsRebuildResult, String> {
    super::global_search::index_integrity::rebuild_messages_fts(&db_state.pool).await
}
