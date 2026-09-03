use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug)]
pub(super) enum DatabaseOpenError {
    ConfirmedCorruption(String),
    Unavailable(String),
}

impl DatabaseOpenError {
    fn is_confirmed_corruption(&self) -> bool {
        matches!(self, Self::ConfirmedCorruption(_))
    }
}

impl fmt::Display for DatabaseOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmedCorruption(error) | Self::Unavailable(error) => error.fmt(formatter),
        }
    }
}

pub(super) async fn open_and_check_db(
    connect_options: &sqlx::sqlite::SqliteConnectOptions,
    db_path: &Path,
) -> Result<Pool<Sqlite>, DatabaseOpenError> {
    let mut retry_count = 0_u64;
    let pool = loop {
        match SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(connect_options.clone())
            .await
        {
            Ok(pool) => break pool,
            Err(error) => {
                retry_count += 1;
                if retry_count >= 3 {
                    return Err(classify_database_error(
                        format!("数据库连接重试失败 (已重试 {retry_count} 次): {error}"),
                        &error,
                    ));
                }
                log::warn!(
                    "[DBManager] Connection failed: {}. Retrying in {}ms... (Attempt {})",
                    error,
                    retry_count * 50,
                    retry_count
                );
                tokio::time::sleep(Duration::from_millis(retry_count * 50)).await;
            }
        }
    };

    if db_path.exists() {
        match check_integrity(&pool).await {
            Ok(true) => {}
            Ok(false) => {
                best_effort_checkpoint(&pool).await;
                pool.close().await;
                return Err(DatabaseOpenError::ConfirmedCorruption(
                    "PRAGMA quick_check(1) failed".to_string(),
                ));
            }
            Err(error) => {
                let classified = classify_database_error(
                    "PRAGMA quick_check(1) could not complete".to_string(),
                    &error,
                );
                if classified.is_confirmed_corruption() {
                    best_effort_checkpoint(&pool).await;
                }
                pool.close().await;
                return Err(classified);
            }
        }
    }
    Ok(pool)
}

async fn best_effort_checkpoint(pool: &Pool<Sqlite>) {
    match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await
    {
        Ok(_) => log::info!("[DBManager] WAL checkpoint completed before database recovery."),
        Err(error) => log::warn!(
            "[DBManager] WAL checkpoint before database recovery failed; preserving sidecars: {}",
            error
        ),
    }
}

async fn check_integrity(pool: &Pool<Sqlite>) -> Result<bool, sqlx::Error> {
    let result = sqlx::query_scalar::<_, String>("PRAGMA quick_check(1)")
        .fetch_one(pool)
        .await?;
    Ok(result.eq_ignore_ascii_case("ok"))
}

pub(super) const SQLITE_BUSY: i32 = 5;
pub(super) const SQLITE_LOCKED: i32 = 6;
pub(super) const SQLITE_IOERR: i32 = 10;
pub(super) const SQLITE_CORRUPT: i32 = 11;
pub(super) const SQLITE_NOTADB: i32 = 26;

fn classify_database_error(context: String, error: &sqlx::Error) -> DatabaseOpenError {
    let message = format!("{context}: {error}");
    if is_confirmed_corruption_code(sqlite_primary_code(error)) {
        return DatabaseOpenError::ConfirmedCorruption(message);
    }
    match sqlite_primary_code(error) {
        Some(SQLITE_BUSY | SQLITE_LOCKED | SQLITE_IOERR) => DatabaseOpenError::Unavailable(message),
        _ => DatabaseOpenError::Unavailable(message),
    }
}

pub(super) fn is_confirmed_corruption_code(code: Option<i32>) -> bool {
    matches!(code, Some(SQLITE_CORRUPT | SQLITE_NOTADB))
}

fn sqlite_primary_code(error: &sqlx::Error) -> Option<i32> {
    let sqlx::Error::Database(database_error) = error else {
        return None;
    };
    database_error
        .code()
        .and_then(|code| code.parse::<i32>().ok())
        .map(|code| code & 0xff)
}

pub(super) fn archive_corrupt_db(db_path: &Path) -> Result<(), String> {
    let archive_path = next_archive_path(db_path)?;
    let files = [
        (db_path.to_path_buf(), archive_path.clone()),
        (
            sidecar_path(db_path, "-wal"),
            sidecar_path(&archive_path, "-wal"),
        ),
        (
            sidecar_path(db_path, "-shm"),
            sidecar_path(&archive_path, "-shm"),
        ),
    ];

    let mut moved = Vec::new();
    for (source, target) in files {
        let source_exists = match path_exists(&source) {
            Ok(exists) => exists,
            Err(error) => {
                rollback_archive(&moved);
                return Err(error);
            }
        };
        if !source_exists {
            if source == db_path {
                return Err("主数据库文件不存在，拒绝创建替代数据库".to_string());
            }
            continue;
        }
        if let Err(error) = fs::rename(&source, &target) {
            rollback_archive(&moved);
            return Err(format!("归档文件移动失败: {error}"));
        }
        moved.push((source, target));
    }

    log::warn!(
        "[DBManager] Archived corrupt database set at {:?} (WAL/SHM preserved when present)",
        archive_path
    );
    Ok(())
}

fn next_archive_path(db_path: &Path) -> Result<PathBuf, String> {
    let file_name = db_path
        .file_name()
        .ok_or_else(|| "无法确定数据库文件名，拒绝归档".to_string())?
        .to_string_lossy();
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let timestamp = chrono::Utc::now().timestamp_millis();
    for attempt in 0..1000_u32 {
        let suffix = if attempt == 0 {
            format!("{file_name}.corrupt.{timestamp}")
        } else {
            format!("{file_name}.corrupt.{timestamp}.{attempt}")
        };
        let candidate = parent.join(suffix);
        if !path_exists(&candidate)?
            && !path_exists(&sidecar_path(&candidate, "-wal"))?
            && !path_exists(&sidecar_path(&candidate, "-shm"))?
        {
            return Ok(candidate);
        }
    }
    Err("归档目标已存在，无法安全生成唯一名称".to_string())
}

pub(super) fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = db_path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("无法检查归档路径: {error}")),
    }
}

fn rollback_archive(moved: &[(PathBuf, PathBuf)]) {
    for (source, target) in moved.iter().rev() {
        if let Err(error) = fs::rename(target, source) {
            log::error!(
                "[DBManager] Failed to roll back database archive move; recovery remains closed: {}",
                error
            );
        }
    }
}
