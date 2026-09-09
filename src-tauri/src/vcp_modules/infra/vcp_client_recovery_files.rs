use crate::vcp_modules::chat::topic_types::MessageKey;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "vcp_client_recovery_cleanup.rs"]
mod cleanup;
#[path = "vcp_client_recovery_file_format.rs"]
mod format;

static NEXT_CLAIM_SUFFIX: AtomicU64 = AtomicU64::new(1);
pub(super) use cleanup::cleanup_recovery_cleanup_debt;
pub(crate) use cleanup::cleanup_recovery_cleanup_debt_on_startup;
pub(super) use cleanup::enqueue_cleanup_debt;
pub(super) use format::{
    find_claimed_files, parse_claim_generation, path_exists, path_is_file,
    read_recovery_generation, read_recovery_payload, remove_claimed_file,
    stable_stream_identity_token,
};

/// 恢复文件从原路径改名到 claimed 路径后的事务守卫。
/// 数据库终结成功后，即使清理 claimed 文件失败，也不能回滚已提交终态。
pub(super) struct RecoveryFileClaim {
    original_path: PathBuf,
    claimed_path: PathBuf,
    generation: Option<u64>,
    committed: bool,
    cleanup_pending: bool,
}

impl RecoveryFileClaim {
    pub(super) fn new(original_path: PathBuf, claimed_path: PathBuf) -> Self {
        let generation = parse_claim_generation(&claimed_path);
        Self::with_generation(original_path, claimed_path, generation)
    }

    fn with_generation(
        original_path: PathBuf,
        claimed_path: PathBuf,
        generation: Option<u64>,
    ) -> Self {
        Self {
            original_path,
            claimed_path,
            generation,
            committed: false,
            cleanup_pending: false,
        }
    }

    pub(super) fn claimed_path(&self) -> &Path {
        &self.claimed_path
    }

    pub(super) fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// 提交恢复事务；清理失败记录为欠账，不能伪报数据库未提交。
    pub(super) fn commit(&mut self) -> Result<(), String> {
        match std::fs::remove_file(&self.claimed_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                self.cleanup_pending = true;
                if let Err(debt_error) = cleanup::write_cleanup_debt(&self.claimed_path) {
                    log::error!(
                        "[VCPClient] 恢复清理欠账记录失败：error={debt_error}; result=error"
                    );
                }
                log::error!(
                    "[VCPClient] 恢复文件已提交但清理失败，保留清理欠账：error={error}; result=pending"
                );
            }
        }
        self.committed = true;
        if !self.cleanup_pending {
            cleanup::remove_cleanup_debt(&self.claimed_path);
        }
        Ok(())
    }

    /// 恢复终结事务已提交时，先幂等 unlink，再删除同事务写入的 outbox。
    pub(super) async fn commit_with_pool(
        &mut self,
        pool: &sqlx::Pool<sqlx::Sqlite>,
    ) -> Result<(), String> {
        match std::fs::remove_file(&self.claimed_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                self.cleanup_pending = true;
                if let Err(marker_error) = cleanup::write_cleanup_debt(&self.claimed_path) {
                    return Err(format!(
                        "恢复文件已提交但清理欠账记录失败: {marker_error}; 原始清理错误: {error}"
                    ));
                }
                log::error!("恢复文件已提交但清理失败，保留数据库 outbox：result=pending");
            }
        }
        self.committed = true;
        if !self.cleanup_pending {
            cleanup::remove_cleanup_debt_db(pool, &self.claimed_path).await?;
            cleanup::remove_cleanup_debt(&self.claimed_path);
        }
        Ok(())
    }

    pub(super) fn cleanup_pending(&self) -> bool {
        self.cleanup_pending
    }

    /// 回滚时绝不覆盖 replacement 写入的 canonical 文件。
    pub(super) fn rollback(&mut self) -> Result<(), String> {
        if self.committed {
            return Ok(());
        }
        if path_is_file(&self.original_path)? {
            log::warn!(
                "[VCPClient] 恢复回滚发现新 canonical 文件，保留新文件并放弃旧 claim：result=kept_new"
            );
            return Ok(());
        }
        match std::fs::rename(&self.claimed_path, &self.original_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("恢复原恢复文件失败: {error}")),
        }
    }
}

impl Drop for RecoveryFileClaim {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(error) = self.rollback() {
                log::error!("[VCPClient] 恢复文件事务回滚失败: {error}");
            }
        }
    }
}

pub(super) struct RecoveryFileCandidate {
    canonical_path: PathBuf,
    source_path: PathBuf,
    generation: Option<u64>,
}

impl RecoveryFileCandidate {
    fn from_canonical(path: PathBuf) -> Self {
        Self {
            canonical_path: path.clone(),
            source_path: path,
            generation: None,
        }
    }

    fn from_claimed(canonical_path: PathBuf, source_path: PathBuf) -> Self {
        Self {
            canonical_path,
            generation: parse_claim_generation(&source_path),
            source_path,
        }
    }

    fn is_claimed(&self) -> bool {
        self.source_path != self.canonical_path
    }

    fn generation(&self) -> Option<u64> {
        self.generation
    }
}

pub(super) async fn claim_recovery_file_in_transition(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &Path,
    key: &MessageKey,
    msg_id: &str,
    epoch: u64,
) -> Result<Option<RecoveryFileClaim>, String> {
    let Some(candidate) = find_recovery_file(pool, cache_dir, key, msg_id).await? else {
        return Ok(None);
    };
    if candidate.is_claimed() {
        let generation = candidate.generation();
        return Ok(Some(RecoveryFileClaim::with_generation(
            candidate.canonical_path,
            candidate.source_path,
            generation,
        )));
    }
    let generation = read_recovery_generation(&candidate.source_path)?;
    let claimed_path = next_claimed_path(&candidate.canonical_path, generation, epoch)?;
    match std::fs::rename(&candidate.source_path, &claimed_path) {
        Ok(()) => Ok(Some(RecoveryFileClaim::with_generation(
            candidate.canonical_path,
            claimed_path,
            generation,
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("占用恢复文件失败: {error}")),
    }
}

fn next_claimed_path(path: &Path, generation: Option<u64>, epoch: u64) -> Result<PathBuf, String> {
    let mut suffix = NEXT_CLAIM_SUFFIX.fetch_add(1, Ordering::Relaxed);
    loop {
        let mut claimed = path.to_path_buf();
        let marker = match generation {
            Some(generation) => format!(
                "claimed.g{generation}.e{epoch}.p{}.s{suffix}",
                std::process::id()
            ),
            None => format!("discarded.e{epoch}.p{}.s{suffix}", std::process::id()),
        };
        claimed.set_extension(marker);
        if !path_exists(&claimed)? {
            return Ok(claimed);
        }
        suffix = suffix.wrapping_add(1);
    }
}

pub(super) async fn find_recovery_file(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    cache_dir: &Path,
    key: &MessageKey,
    msg_id: &str,
) -> Result<Option<RecoveryFileCandidate>, String> {
    let cache = cache_dir.join("sse_cache");
    let token = stable_stream_identity_token(key);
    let canonical = cache.join(format!("sse_recovered_{token}.json"));
    let claimed = find_claimed_files(&cache, &token)?;
    let mut valid_claimed = Vec::new();
    for path in claimed {
        if parse_claim_generation(&path).is_some() {
            valid_claimed.push(path);
        } else {
            log::warn!("[VCPClient] 丢弃缺少 generation 的旧 claimed 文件：result=discarded");
            remove_claimed_file(&path)?;
        }
    }
    if path_is_file(&canonical)? {
        for path in valid_claimed {
            remove_claimed_file(&path)?;
        }
        return Ok(Some(RecoveryFileCandidate::from_canonical(canonical)));
    }
    if valid_claimed.len() > 1 {
        valid_claimed.sort();
        for path in valid_claimed {
            remove_claimed_file(&path)?;
        }
        return Err("恢复缓存存在多个 claimed 文件，已安全清理并拒绝恢复".to_string());
    }
    if let Some(claimed) = valid_claimed.into_iter().next() {
        return Ok(Some(RecoveryFileCandidate::from_claimed(
            canonical, claimed,
        )));
    }
    let legacy = cache.join(format!(
        "sse_recovered_{}.json",
        crate::vcp_modules::infra::utils::calculate_sha256(msg_id.as_bytes())
    ));
    if path_is_file(&legacy)? && super::legacy_recovery_file_is_unambiguous(pool, key).await? {
        return Ok(Some(RecoveryFileCandidate::from_canonical(legacy)));
    }
    Ok(None)
}
