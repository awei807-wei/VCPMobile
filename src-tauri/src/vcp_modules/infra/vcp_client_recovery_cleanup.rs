use crate::vcp_modules::chat::topic_types::MessageKey;
use sqlx::{Sqlite, Transaction};
use std::path::{Path, PathBuf};

const CLEANUP_DEBT_DIR: &str = "sse_cleanup_debt";
const MAX_CLEANUP_ITEMS: usize = 128;

pub(crate) async fn enqueue_cleanup_debt(
    tx: &mut Transaction<'_, Sqlite>,
    cache_dir: &Path,
    claimed_path: &Path,
    key: &MessageKey,
    generation: u64,
) -> Result<(), String> {
    if !is_safe_cleanup_target(cache_dir, claimed_path, generation) {
        return Err("恢复清理义务路径不在 app cache/sse_cache 内".to_string());
    }
    let generation = i64::try_from(generation)
        .map_err(|_| "恢复清理欠账 generation 超出 SQLite 整数范围".to_string())?;
    let path = claimed_path.to_string_lossy().to_string();
    sqlx::query(
        "INSERT INTO recovery_cleanup_outbox
            (claimed_path, owner_type, owner_id, topic_id, msg_id, helper_generation, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(claimed_path) DO NOTHING",
    )
    .bind(&path)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .bind(generation)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("持久化恢复清理义务失败: {error}"))?;
    let stored: Option<(String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT owner_type, owner_id, topic_id, msg_id, claimed_path, helper_generation
         FROM recovery_cleanup_outbox WHERE claimed_path = ?",
    )
    .bind(&path)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("校验恢复清理义务失败: {error}"))?;
    let Some((owner_type, owner_id, topic_id, msg_id, stored_path, stored_generation)) = stored
    else {
        return Err("恢复清理义务写入后不可见".to_string());
    };
    if stored_path != path
        || owner_type != key.topic.owner_type
        || owner_id != key.topic.owner_id
        || topic_id != key.topic.topic_id
        || msg_id != key.msg_id
        || stored_generation != generation
    {
        return Err("恢复清理义务身份或 generation 冲突".to_string());
    }
    Ok(())
}

pub(crate) async fn remove_cleanup_debt_db(
    pool: &sqlx::Pool<Sqlite>,
    claimed_path: &Path,
) -> Result<(), String> {
    sqlx::query("DELETE FROM recovery_cleanup_outbox WHERE claimed_path = ?")
        .bind(claimed_path.to_string_lossy().as_ref())
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|error| format!("删除恢复清理义务失败: {error}"))
}

pub(super) fn write_cleanup_debt(claimed_path: &Path) -> Result<(), String> {
    let cache_dir = claimed_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "恢复清理欠账缺少缓存目录".to_string())?;
    let debt_path = cleanup_debt_path(cache_dir, claimed_path);
    let debt_dir = debt_path
        .parent()
        .ok_or_else(|| "恢复清理欠账目录无效".to_string())?;
    std::fs::create_dir_all(debt_dir).map_err(|error| format!("创建清理欠账目录失败: {error}"))?;
    let temporary = debt_path.with_extension("tmp");
    std::fs::write(&temporary, claimed_path.to_string_lossy().as_bytes())
        .map_err(|error| format!("写入清理欠账失败: {error}"))?;
    std::fs::rename(&temporary, &debt_path).map_err(|error| format!("发布清理欠账失败: {error}"))
}

pub(super) fn remove_cleanup_debt(claimed_path: &Path) {
    let Some(cache_dir) = claimed_path.parent().and_then(Path::parent) else {
        return;
    };
    let debt_path = cleanup_debt_path(cache_dir, claimed_path);
    let _ = std::fs::remove_file(debt_path);
}

/// 启动维护只处理已持久化 outbox、旧 debt 和明确 discarded 的残留。
/// claimed 文件没有 outbox 时仍可能属于尚未完成的恢复，不能猜测清理。
pub(crate) async fn cleanup_recovery_cleanup_debt(
    cache_dir: &Path,
    pool: &sqlx::Pool<Sqlite>,
) -> Result<(), String> {
    cleanup_outbox(cache_dir, pool).await?;
    cleanup_filesystem_debt(cache_dir)?;
    cleanup_discarded_files(cache_dir)?;
    Ok(())
}

/// 在数据库就绪后独立执行恢复清理扫描；清扫失败只记录，不能阻断核心启动。
pub(crate) async fn cleanup_recovery_cleanup_debt_on_startup(
    cache_dir: &Path,
    pool: &sqlx::Pool<Sqlite>,
) {
    match cleanup_recovery_cleanup_debt(cache_dir, pool).await {
        Ok(()) => log::info!("[VCPClient] 启动恢复清理扫描完成。"),
        Err(_error) => log::error!("[VCPClient] 启动恢复清理扫描失败，核心启动继续：result=error"),
    }
}

async fn cleanup_outbox(cache_dir: &Path, pool: &sqlx::Pool<Sqlite>) -> Result<(), String> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT claimed_path, helper_generation FROM recovery_cleanup_outbox
         ORDER BY created_at, claimed_path LIMIT ?",
    )
    .bind(i64::try_from(MAX_CLEANUP_ITEMS).unwrap_or(128))
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取恢复清理义务失败: {error}"))?;
    for (path, generation) in rows {
        let target = PathBuf::from(path);
        let generation = match u64::try_from(generation) {
            Ok(generation) if generation > 0 => generation,
            _ => {
                log::error!("拒绝无效恢复清理义务 generation：result=rejected");
                continue;
            }
        };
        if !is_safe_cleanup_target(cache_dir, &target, generation) {
            log::error!("拒绝越界恢复清理义务：result=rejected");
            continue;
        }
        if let Err(_error) = super::remove_claimed_file(&target) {
            log::warn!("恢复清理义务仍待处理：result=pending");
            continue;
        }
        remove_cleanup_debt_db(pool, &target).await?;
    }
    Ok(())
}

fn cleanup_filesystem_debt(cache_dir: &Path) -> Result<(), String> {
    let debt_dir = cache_dir.join(CLEANUP_DEBT_DIR);
    let entries = match std::fs::read_dir(&debt_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取恢复清理欠账失败: {error}")),
    };
    for (index, entry) in entries.enumerate() {
        if index >= MAX_CLEANUP_ITEMS {
            break;
        }
        let marker = entry
            .map_err(|error| format!("读取清理欠账条目失败: {error}"))?
            .path();
        let target = match std::fs::read_to_string(&marker) {
            Ok(path) => PathBuf::from(path),
            Err(_error) => {
                log::warn!("忽略损坏清理欠账：result=discarded");
                continue;
            }
        };
        if !is_safe_cleanup_target(cache_dir, &target, 0) {
            log::warn!("拒绝越界恢复清理欠账：result=rejected");
            continue;
        }
        if let Err(_error) = super::remove_claimed_file(&target) {
            log::warn!("清理恢复欠账仍失败：result=pending");
            continue;
        }
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("删除恢复清理欠账标记失败: {error}")),
        }
    }
    Ok(())
}

fn cleanup_discarded_files(cache_dir: &Path) -> Result<(), String> {
    let sse_cache = cache_dir.join("sse_cache");
    let entries = match std::fs::read_dir(&sse_cache) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取恢复缓存目录失败: {error}")),
    };
    for (index, entry) in entries.enumerate() {
        if index >= MAX_CLEANUP_ITEMS {
            break;
        }
        let path = entry
            .map_err(|error| format!("读取恢复缓存条目失败: {error}"))?
            .path();
        let name = path.file_name().and_then(|value| value.to_str());
        if name
            .is_some_and(|name| name.starts_with("sse_recovered_") && name.contains(".discarded."))
        {
            super::remove_claimed_file(&path)?;
        }
    }
    Ok(())
}

fn is_safe_cleanup_target(cache_dir: &Path, target: &Path, generation: u64) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    if parent != cache_dir.join("sse_cache") {
        return false;
    }
    let valid_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("sse_recovered_")
                && (name.contains(".claimed.") || name.contains(".discarded."))
        });
    if !valid_name {
        return false;
    }
    if generation == 0 {
        return true;
    }
    target
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(&format!(".claimed.g{generation}.")))
}

fn cleanup_debt_path(cache_dir: &Path, claimed_path: &Path) -> PathBuf {
    let token = crate::vcp_modules::infra::utils::calculate_sha256(
        claimed_path.to_string_lossy().as_bytes(),
    );
    cache_dir
        .join(CLEANUP_DEBT_DIR)
        .join(format!("{token}.json"))
}

#[cfg(test)]
#[path = "vcp_client_recovery_cleanup_tests.rs"]
mod tests;
