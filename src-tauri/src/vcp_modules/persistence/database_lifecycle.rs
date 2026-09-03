use sqlx::{Connection, Pool, Sqlite};
use std::{fs, path::PathBuf, time::Duration};
use tauri::{AppHandle, Emitter, Manager};

use super::migration_runner::run_migrations;

#[path = "database_recovery.rs"]
mod database_recovery;
use database_recovery::{archive_corrupt_db, open_and_check_db, DatabaseOpenError};
#[cfg(test)]
use database_recovery::{
    is_confirmed_corruption_code, sidecar_path, SQLITE_BUSY, SQLITE_CORRUPT, SQLITE_IOERR,
    SQLITE_LOCKED, SQLITE_NOTADB,
};

pub(super) async fn init_db(app_handle: &AppHandle) -> Result<(Pool<Sqlite>, PathBuf), String> {
    let db_path = resolve_database_path(app_handle)?;
    let connect_options = database_connect_options(&db_path);
    let pool = open_database_with_recovery(&connect_options, &db_path).await?;

    run_migrations(&pool).await?;
    let pool = upgrade_page_size_if_needed(app_handle, pool, &connect_options, &db_path).await?;
    normalize_message_content(&pool).await?;
    sync_system_preset_rules(&pool).await?;

    Ok((pool, db_path))
}

fn resolve_database_path(app_handle: &AppHandle) -> Result<PathBuf, String> {
    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|error| format!("Config dir failed: {error}"))?;
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).map_err(|error| format!("Create dir failed: {error}"))?;
    }

    let db_path = config_dir.join("vcp_avatar.db");
    log::info!("[DBManager] Initializing SQLite at: {:?}", db_path);
    Ok(db_path)
}

fn database_connect_options(db_path: &std::path::Path) -> sqlx::sqlite::SqliteConnectOptions {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30))
        .pragma("mmap_size", "268435456")
        .pragma("temp_store", "2")
        .pragma("page_size", "16384")
        .pragma("cache_size", "-8000")
        .pragma("auto_vacuum", "2")
        .pragma("foreign_keys", "1")
}

async fn open_database_with_recovery(
    connect_options: &sqlx::sqlite::SqliteConnectOptions,
    db_path: &std::path::Path,
) -> Result<Pool<Sqlite>, String> {
    match open_and_check_db(connect_options, db_path).await {
        Ok(pool) => Ok(pool),
        Err(DatabaseOpenError::ConfirmedCorruption(error)) => {
            log::warn!(
                "[DBManager] Database open/integrity check failed: {}. Attempting self-healing...",
                error
            );
            archive_corrupt_db(db_path).map_err(|archive_error| {
                log::error!(
                    "[DBManager] Corrupt database archive failed; refusing to create a replacement: {}",
                    archive_error
                );
                format!("数据库损坏且归档失败，已拒绝创建新数据库: {archive_error}")
            })?;
            open_and_check_db(connect_options, db_path)
                .await
                .map_err(|retry_error| format!("数据库损坏且重建失败: {retry_error}"))
        }
        Err(DatabaseOpenError::Unavailable(error)) => {
            log::error!(
                "[DBManager] Database unavailable; recovery skipped and original files were preserved: {}",
                error
            );
            Err(format!("数据库暂时不可用，未执行归档或重建: {error}"))
        }
    }
}

async fn upgrade_page_size_if_needed(
    app_handle: &AppHandle,
    pool: Pool<Sqlite>,
    connect_options: &sqlx::sqlite::SqliteConnectOptions,
    db_path: &std::path::Path,
) -> Result<Pool<Sqlite>, String> {
    let page_size = sqlx::query_scalar::<_, i32>("PRAGMA page_size")
        .fetch_one(&pool)
        .await
        .unwrap_or(4096);
    if page_size == 16384 {
        return Ok(pool);
    }

    log::info!(
        "[DBManager] Legacy page_size {} detected. Running page size VACUUM optimization...",
        page_size
    );
    announce_page_size_optimization(app_handle).await;
    rebuild_with_16k_pages(pool, db_path).await;
    let reopened = open_and_check_db(connect_options, db_path)
        .await
        .map_err(|error| format!("重建连接池失败: {error}"))?;
    reset_core_to_initializing(app_handle).await;
    Ok(reopened)
}

async fn announce_page_size_optimization(app_handle: &AppHandle) {
    let lifecycle =
        app_handle.state::<crate::vcp_modules::infra::lifecycle_state::LifecycleState>();
    *lifecycle.status.write().await =
        crate::vcp_modules::infra::lifecycle_state::CoreStatus::Optimizing;
    *lifecycle.status_message.write().await = "正在优化数据库存储以提高运行效率...".to_string();
    let _ = app_handle.emit(
        "vcp-system-event",
        serde_json::json!({
            "type": "vcp-core-status",
            "status": "optimizing",
            "message": "正在优化数据库存储以提高运行效率...",
            "source": "Core"
        }),
    );
}

async fn rebuild_with_16k_pages(pool: Pool<Sqlite>, db_path: &std::path::Path) {
    pool.close().await;
    let temp_options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete);

    match sqlx::sqlite::SqliteConnection::connect_with(&temp_options).await {
        Ok(mut connection) => {
            let _ = sqlx::query("PRAGMA page_size = 16384")
                .execute(&mut connection)
                .await;
            if let Err(error) = sqlx::query("VACUUM").execute(&mut connection).await {
                log::error!("[DBManager] Page size VACUUM optimization failed: {error}");
            } else {
                log::info!("[DBManager] Page size successfully upgraded to 16KB.");
            }
            let _ = connection.close().await;
        }
        Err(error) => {
            log::error!(
                "[DBManager] Failed to open temp connection for page size optimization: {error}"
            );
        }
    }
}

async fn reset_core_to_initializing(app_handle: &AppHandle) {
    let lifecycle =
        app_handle.state::<crate::vcp_modules::infra::lifecycle_state::LifecycleState>();
    *lifecycle.status.write().await =
        crate::vcp_modules::infra::lifecycle_state::CoreStatus::Initializing;
}

async fn normalize_message_content(pool: &Pool<Sqlite>) -> Result<(), String> {
    let normalized = super::message_content_storage::normalize_legacy_message_content(pool)
        .await
        .map_err(|error| {
            format!("[DBManager] Failed to normalize message content storage: {error}")
        })?;
    if normalized > 0 {
        log::info!(
            "[DBManager] Normalized {} legacy TEXT message bodies to compressed BLOB storage.",
            normalized
        );
    }
    Ok(())
}

async fn sync_system_preset_rules(pool: &Pool<Sqlite>) -> Result<(), String> {
    crate::vcp_modules::chat::context_injection::sync_system_preset_rules(pool)
        .await
        .map_err(|error| format!("[DBManager] Failed to sync preset rules: {error}"))
}

#[cfg(test)]
#[path = "database_lifecycle_tests.rs"]
mod tests;
