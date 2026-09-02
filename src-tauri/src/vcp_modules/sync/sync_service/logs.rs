use super::errors::encode_sync_command_error;
use super::types::SyncState;
use crate::vcp_modules::sync_logger::{redact_sync_diagnostic, LogLevel};
use std::path::Path;
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

pub(crate) fn emit_sync_log<R: Runtime>(app_handle: &AppHandle<R>, level: &str, message: &str) {
    let sync_state = app_handle.state::<SyncState>();
    if let Some(logger_arc) = sync_state
        .current_logger
        .read()
        .ok()
        .and_then(|guard| guard.clone())
    {
        if let Ok(mut logger) = logger_arc.lock() {
            logger.log(parse_log_level(level), "sync", message);
            return;
        }
    }
    log::log!(
        parse_rust_log_level(level),
        "[Sync] [{}] {}",
        level,
        redact_sync_diagnostic(message)
    );
}

fn parse_log_level(level: &str) -> LogLevel {
    match level {
        "trace" => LogLevel::Trace,
        "debug" => LogLevel::Debug,
        "error" => LogLevel::Error,
        "warn" | "warning" => LogLevel::Warning,
        _ => LogLevel::Info,
    }
}

fn parse_rust_log_level(level: &str) -> log::Level {
    match level {
        "trace" => log::Level::Trace,
        "debug" => log::Level::Debug,
        "error" => log::Level::Error,
        "warn" | "warning" => log::Level::Warn,
        _ => log::Level::Info,
    }
}

pub(crate) fn emit_operator_sync_log<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: u64,
    level: &str,
    message: &str,
) {
    let _ = app_handle.emit(
        "vcp-log",
        serde_json::json!({
            "id": format!("{}_{}", level, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            "level": level,
            "category": "sync",
            "audience": "operator",
            "sessionId": session_id,
            "message": message,
        }),
    );
}

#[derive(Debug, serde::Serialize)]
pub struct SyncLogFileInfo {
    pub filename: String,
    pub created_at: u64,
    pub size_bytes: u64,
}

#[tauri::command]
pub async fn list_sync_log_files(app: AppHandle) -> Result<Vec<SyncLogFileInfo>, String> {
    let log_dir = sync_log_dir(&app, "SYNC_LOG_LIST_FAILED")?;
    if !log_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&log_dir)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string()))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string()))?
    {
        let metadata = entry.metadata().await.map_err(|error| {
            encode_sync_command_error("SYNC_LOG_LIST_FAILED", &error.to_string())
        })?;
        if metadata.is_file() {
            entries.push(SyncLogFileInfo {
                filename: entry.file_name().to_string_lossy().to_string(),
                created_at: file_timestamp(&metadata),
                size_bytes: metadata.len(),
            });
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
    Ok(entries)
}

fn file_timestamp(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

#[tauri::command]
pub async fn get_sync_session_log_path(
    state: State<'_, SyncState>,
) -> Result<Option<String>, String> {
    Ok(state.current_log_path.read().await.clone())
}

#[tauri::command]
pub async fn read_sync_log_file(app: AppHandle, filename: String) -> Result<String, String> {
    let log_dir = sync_log_dir(&app, "SYNC_LOG_READ_FAILED")?;
    let file_path = log_dir.join(&filename);
    let canonical_dir = log_dir
        .canonicalize()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?;
    let canonical_file = file_path
        .canonicalize()
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))?;
    if !canonical_file.starts_with(&canonical_dir) {
        return Err(encode_sync_command_error(
            "SYNC_LOG_PATH_INVALID",
            "Requested sync log is outside the sync log directory",
        ));
    }
    tokio::fs::read_to_string(&canonical_file)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_READ_FAILED", &error.to_string()))
}

fn sync_log_dir(app: &AppHandle, code: &str) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_log_dir()
        .map(|path| path.join("sync_logs"))
        .map_err(|error| encode_sync_command_error(code, &error.to_string()))
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLogCleanupResult {
    pub removed: u32,
    pub failed: u32,
}

pub(crate) fn count_log_removal(
    result: std::io::Result<()>,
    removed: &mut u32,
    failed: &mut u32,
) -> Option<std::io::Error> {
    match result {
        Ok(()) => {
            *removed += 1;
            None
        }
        Err(error) => {
            *failed += 1;
            Some(error)
        }
    }
}

#[tauri::command]
pub async fn clear_old_sync_logs(
    app: AppHandle,
    keep_days: u32,
) -> Result<SyncLogCleanupResult, String> {
    let log_dir = sync_log_dir(&app, "SYNC_LOG_CLEAR_FAILED")?;
    if !log_dir.exists() {
        return Ok(SyncLogCleanupResult {
            removed: 0,
            failed: 0,
        });
    }
    let cutoff = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(keep_days as u64 * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    remove_old_logs(&log_dir, cutoff).await
}

async fn remove_old_logs(
    log_dir: &Path,
    cutoff: SystemTime,
) -> Result<SyncLogCleanupResult, String> {
    let mut removed = 0;
    let mut failed = 0;
    let mut read_dir = tokio::fs::read_dir(log_dir)
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string()))?;
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|error| encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string()))?
    {
        let metadata = entry.metadata().await.map_err(|error| {
            encode_sync_command_error("SYNC_LOG_CLEAR_FAILED", &error.to_string())
        })?;
        if metadata.is_file() && metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH) < cutoff {
            if let Some(error) = count_log_removal(
                tokio::fs::remove_file(entry.path()).await,
                &mut removed,
                &mut failed,
            ) {
                log::warn!(
                    "[SyncLog] Failed to remove old log: {}",
                    redact_sync_diagnostic(&error.to_string())
                );
            }
        }
    }
    Ok(SyncLogCleanupResult { removed, failed })
}
