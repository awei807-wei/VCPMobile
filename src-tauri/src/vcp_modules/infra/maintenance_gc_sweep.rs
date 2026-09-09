use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::super::db::{PathReferenceIndex, PathReferenceState};
use super::scan::scan_managed_root_from_cursor;
use super::secure;
use super::{
    canonical_managed_root, is_managed_attachment_temp, managed_hash_from_file_name,
    remove_indexed_path, scan_cursor_key, should_expire_temp, ManagedAttachmentRoots, RootKind,
    SweepReport, ATTACHMENT_TEMP_GRACE, MAX_SCANNED_FILES_PER_ROOT,
};

/// 受管目录测试扫描所需的引用保护、时钟与分页策略。
pub(super) struct ManagedRootSweepOptions<'a> {
    kind: RootKind,
    retained_paths: &'a HashSet<PathBuf>,
    indexed_hashes: &'a HashSet<String>,
    live_hashes: &'a HashSet<String>,
    now: SystemTime,
    temp_grace: Duration,
    max_entries: usize,
}

impl<'a> ManagedRootSweepOptions<'a> {
    pub(super) fn new(
        kind: RootKind,
        retained_paths: &'a HashSet<PathBuf>,
        indexed_hashes: &'a HashSet<String>,
        live_hashes: &'a HashSet<String>,
        now: SystemTime,
        temp_grace: Duration,
        max_entries: usize,
    ) -> Self {
        Self {
            kind,
            retained_paths,
            indexed_hashes,
            live_hashes,
            now,
            temp_grace,
            max_entries,
        }
    }
}

/// 清理受管目录树中的孤立 CAS 与过期上传临时文件。
///
/// `retained_paths` 是从 attachments 表的精确路径字段建立的集合，`indexed_hashes` 和
/// `live_hashes` 则用于 malformed/非标准路径的保守保护：只要数据库仍知道这个 hash，
/// 就不因目录文件名猜测而删除它。
pub(super) async fn sweep_managed_root(
    root: &Path,
    options: ManagedRootSweepOptions<'_>,
) -> SweepReport {
    let ManagedRootSweepOptions {
        kind,
        retained_paths,
        indexed_hashes,
        live_hashes,
        now,
        temp_grace,
        max_entries,
    } = options;
    let Ok(canonical_root) = canonical_managed_root(root) else {
        return SweepReport::default();
    };
    let page = scan_managed_root_from_cursor(&canonical_root, "", max_entries).await;
    let mut report = SweepReport {
        inspected: page.candidates.len(),
        has_more: page.has_more,
        ..SweepReport::default()
    };
    let context = RegularSweepContext {
        canonical_root: &canonical_root,
        kind,
        retained_paths,
        indexed_hashes,
        live_hashes,
        now,
        temp_grace,
    };
    for candidate in page.candidates {
        sweep_regular_file(&context, &candidate.path, candidate.metadata, &mut report).await;
    }
    report
}

/// 在附件索引事务提交后扫描一轮幽灵文件，并按精确路径保护已索引文件。
pub async fn sweep_root_after_commit(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    root: &Path,
    kind: RootKind,
) -> Result<SweepReport, String> {
    sweep_root_after_commit_with_clock(
        connection,
        roots,
        root,
        kind,
        SystemTime::now(),
        ATTACHMENT_TEMP_GRACE,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn sweep_root_after_commit_for_test(
    connection: &mut sqlx::SqliteConnection,
    root: &Path,
    kind: RootKind,
    path_index: &PathReferenceIndex,
    now: SystemTime,
    temp_grace: Duration,
) -> Result<SweepReport, String> {
    let canonical_root = canonical_managed_root(root)?;
    let cursor_key = scan_cursor_key(kind);
    let persisted_cursor = super::super::db::read_gc_cursor(connection, cursor_key).await?;
    let page = scan_managed_root_from_cursor(
        &canonical_root,
        &persisted_cursor,
        MAX_SCANNED_FILES_PER_ROOT,
    )
    .await;
    let mut report = SweepReport {
        inspected: page.candidates.len(),
        has_more: page.has_more,
        ..SweepReport::default()
    };
    let context = CommittedSweepContext {
        root,
        canonical_root: &canonical_root,
        kind,
        path_index,
        now,
        temp_grace,
    };
    inspect_committed_candidates(&context, page.candidates, &mut report).await?;
    let next_cursor = if page.has_more {
        page.next_cursor
    } else {
        String::new()
    };
    super::super::db::write_gc_cursor(connection, cursor_key, &next_cursor).await?;
    Ok(report)
}

/// Test-only structural seam: the candidate page is inspected with an
/// already-loaded path snapshot and no database handle. This keeps the
/// no-per-candidate-SQL property directly observable in tests.
#[cfg(test)]
pub(crate) async fn sweep_snapshot_page_for_test(
    root: &Path,
    kind: RootKind,
    path_index: &PathReferenceIndex,
    max_entries: usize,
) -> Result<SweepReport, String> {
    let canonical_root = canonical_managed_root(root)?;
    let page = scan_managed_root_from_cursor(&canonical_root, "", max_entries).await;
    let mut report = SweepReport {
        inspected: page.candidates.len(),
        has_more: page.has_more,
        ..SweepReport::default()
    };
    let context = CommittedSweepContext {
        root,
        canonical_root: &canonical_root,
        kind,
        path_index,
        now: SystemTime::now(),
        temp_grace: ATTACHMENT_TEMP_GRACE,
    };
    inspect_committed_candidates(&context, page.candidates, &mut report).await?;
    Ok(report)
}

async fn sweep_root_after_commit_with_clock(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    root: &Path,
    kind: RootKind,
    now: SystemTime,
    temp_grace: Duration,
) -> Result<SweepReport, String> {
    let canonical_root = canonical_managed_root(root)?;
    let cursor_key = scan_cursor_key(kind);
    let persisted_cursor = super::super::db::read_gc_cursor(connection, cursor_key).await?;
    let page = scan_managed_root_from_cursor(
        &canonical_root,
        &persisted_cursor,
        MAX_SCANNED_FILES_PER_ROOT,
    )
    .await;
    let mut report = SweepReport {
        inspected: page.candidates.len(),
        has_more: page.has_more,
        ..SweepReport::default()
    };
    let path_keys = page
        .candidates
        .iter()
        .map(|candidate| (kind, candidate.path.clone()))
        .collect::<Vec<_>>();
    let hashes = page
        .candidates
        .iter()
        .filter_map(|candidate| candidate.path.file_name()?.to_str())
        .filter_map(|name| managed_hash_from_file_name(name, kind))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let path_index = super::super::db::PathReferenceIndex::load_for_paths(
        connection, roots, &path_keys, &hashes,
    )
    .await?;
    let context = CommittedSweepContext {
        root,
        canonical_root: &canonical_root,
        kind,
        path_index: &path_index,
        now,
        temp_grace,
    };
    inspect_committed_candidates(&context, page.candidates, &mut report).await?;
    let next_cursor = if page.has_more {
        page.next_cursor
    } else {
        String::new()
    };
    super::super::db::write_gc_cursor(connection, cursor_key, &next_cursor).await?;
    Ok(report)
}

struct CommittedSweepContext<'a> {
    root: &'a Path,
    canonical_root: &'a Path,
    kind: RootKind,
    path_index: &'a PathReferenceIndex,
    now: SystemTime,
    temp_grace: Duration,
}

async fn inspect_committed_candidates(
    context: &CommittedSweepContext<'_>,
    candidates: Vec<super::scan::ManagedFileCandidate>,
    report: &mut SweepReport,
) -> Result<(), String> {
    for candidate in candidates {
        inspect_committed_candidate(context, candidate, report).await?;
    }
    Ok(())
}

async fn inspect_committed_candidate(
    context: &CommittedSweepContext<'_>,
    candidate: super::scan::ManagedFileCandidate,
    report: &mut SweepReport,
) -> Result<(), String> {
    let Some(name) = candidate.path.file_name().and_then(|value| value.to_str()) else {
        return Ok(());
    };
    if is_managed_attachment_temp(name) {
        let Ok(canonical) = std::fs::canonicalize(&candidate.path) else {
            return Ok(());
        };
        if !matches!(
            context
                .path_index
                .path_reference_state(context.root, &canonical),
            Ok(PathReferenceState::Unreferenced)
        ) {
            return Ok(());
        }
        if should_expire_temp(
            candidate.metadata.modified(),
            context.now,
            context.temp_grace,
        ) && remove_indexed_path(context.root, &candidate.path.to_string_lossy())
            .await
            .is_ok_and(|value| value)
        {
            report.removed += 1;
        }
        return Ok(());
    }
    let Some(hash) = managed_hash_from_file_name(name, context.kind) else {
        return Ok(());
    };
    let Ok(canonical) = std::fs::canonicalize(&candidate.path) else {
        return Ok(());
    };
    if !canonical.starts_with(context.canonical_root) {
        return Ok(());
    }
    if !matches!(
        context
            .path_index
            .path_reference_state(context.root, &canonical),
        Ok(PathReferenceState::Unreferenced)
    ) || context.path_index.has_indexed_hash(hash)
        || context.path_index.has_live_reference(hash)
    {
        return Ok(());
    }
    if remove_indexed_path(context.root, &canonical.to_string_lossy())
        .await
        .is_ok_and(|value| value)
    {
        report.removed += 1;
    }
    Ok(())
}

struct RegularSweepContext<'a> {
    canonical_root: &'a Path,
    kind: RootKind,
    retained_paths: &'a HashSet<PathBuf>,
    indexed_hashes: &'a HashSet<String>,
    live_hashes: &'a HashSet<String>,
    now: SystemTime,
    temp_grace: Duration,
}

async fn sweep_regular_file(
    context: &RegularSweepContext<'_>,
    path: &Path,
    metadata: std::fs::Metadata,
    report: &mut SweepReport,
) {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    if is_managed_attachment_temp(name) {
        if should_expire_temp(metadata.modified(), context.now, context.temp_grace)
            && secure::remove_candidate(context.canonical_root, path).is_ok_and(|removed| removed)
        {
            report.removed += 1;
        }
        return;
    }
    let Some(hash) = managed_hash_from_file_name(name, context.kind) else {
        return;
    };
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return;
    };
    if !canonical.starts_with(context.canonical_root)
        || context.retained_paths.contains(&canonical)
        || contains_hash(context.indexed_hashes, hash)
        || contains_hash(context.live_hashes, hash)
    {
        return;
    }
    if secure::remove_candidate(context.canonical_root, &canonical).is_ok_and(|removed| removed) {
        report.removed += 1;
    }
}

fn contains_hash(hashes: &HashSet<String>, candidate: &str) -> bool {
    hashes
        .iter()
        .any(|hash| hash.eq_ignore_ascii_case(candidate))
}
