use super::batch::{validate_record_paths, RecordPaths};
use super::db::{
    clear_unlink_debt, delete_index_if_orphaned, enqueue_unlink, has_unlink_debts,
    load_attachment_page, read_gc_cursor, scrub_deleted_attachment_links, write_gc_cursor,
    AttachmentGcCycleState, IndexedAttachment, PathReferenceIndex, PathReferenceState,
    ATTACHMENT_GC_CURSOR_KEY, ATTACHMENT_GC_CYCLE_KEY, RELATION_GC_CURSOR_KEY,
};
use super::outbox::retry_unlink_outbox_before;
use super::paths::{
    canonical_managed_root, path_may_be_inside_managed_root, relative_path_for_root,
    relative_path_for_validated_root, resolve_reference_target_through_symlinks, root_kind_key,
    validate_reference_path, ManagedAttachmentRoots, RootKind,
};
use super::AttachmentGcReport;
use crate::vcp_modules::infra::utils::is_valid_cas_hash;

#[derive(Debug, Default)]
pub(super) struct GcBatch {
    pub(super) report: AttachmentGcReport,
    pub(super) next_cursor: Option<String>,
    pub(super) first_page: bool,
    pub(super) cycle_state: Option<AttachmentGcCycleState>,
    pub(super) reset_cycle: bool,
    pub(super) page_exhausted: bool,
    pub(super) relation_cursor: Option<i64>,
    pub(super) relation_exhausted: bool,
    pub(super) pending_unlinks: Vec<PendingUnlink>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingUnlink {
    pub(super) root_kind: RootKind,
    pub(super) path: std::path::PathBuf,
}

pub(super) async fn build_gc_batch(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    cursor: &str,
    relation_cursor: i64,
) -> Result<GcBatch, String> {
    let relation_page = scrub_deleted_attachment_links(connection, relation_cursor).await?;
    let mut batch = GcBatch::default();
    batch.first_page = cursor.is_empty();
    let cycle_state = if batch.first_page {
        Some(AttachmentGcCycleState {
            token: uuid::Uuid::new_v4().to_string(),
            cycle_start: crate::vcp_modules::infra::utils::now_millis(),
            next_cursor: String::new(),
        })
    } else {
        serde_json::from_str::<AttachmentGcCycleState>(
            &read_gc_cursor(connection, ATTACHMENT_GC_CYCLE_KEY).await?,
        )
        .ok()
        .filter(|state| {
            !state.token.is_empty() && state.cycle_start > 0 && state.next_cursor == cursor
        })
    };
    let page = load_attachment_page(connection, cursor).await?;
    if !batch.first_page && (cycle_state.is_none() || page.records.is_empty()) {
        batch.reset_cycle = true;
        batch.report.has_more = true;
        batch.report.cursor = None;
        batch.relation_cursor = relation_page.next_cursor;
        batch.relation_exhausted = relation_page.exhausted;
        return Ok(batch);
    }
    let mut cycle_state = cycle_state.expect("validated attachment GC cycle state");
    batch.relation_cursor = relation_page.next_cursor;
    batch.relation_exhausted = relation_page.exhausted;
    let mut path_index =
        PathReferenceIndex::load_for_records(connection, roots, &page.records).await?;
    batch.next_cursor = page.records.last().map(|row| row.hash.clone());
    batch.page_exhausted = !page.has_more;
    cycle_state.next_cursor = batch.next_cursor.clone().unwrap_or_default();
    batch.cycle_state = Some(cycle_state);
    batch.report.has_more = !batch.page_exhausted || !batch.relation_exhausted;
    batch.report.cursor = (!batch.page_exhausted)
        .then(|| batch.next_cursor.clone())
        .flatten();
    for record in &page.records {
        reclaim_record(connection, roots, record, &mut batch, &mut path_index).await?;
    }
    enqueue_pending_unlinks(connection, roots, &batch.pending_unlinks, &path_index).await?;
    Ok(batch)
}

pub(super) async fn build_single_batch(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    hash: &str,
) -> Result<GcBatch, String> {
    let mut batch = GcBatch::default();
    let Some(record) = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT hash, internal_path, thumbnail_path FROM attachments WHERE hash = ?",
    )
    .bind(hash)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|error| format!("读取指定附件索引失败: {error}"))?
    .map(|(hash, internal_path, thumbnail_path)| IndexedAttachment {
        hash,
        internal_path,
        thumbnail_path,
    }) else {
        return Ok(batch);
    };
    let mut path_index =
        PathReferenceIndex::load_for_records(connection, roots, std::slice::from_ref(&record))
            .await?;
    reclaim_record(connection, roots, &record, &mut batch, &mut path_index).await?;
    enqueue_pending_unlinks(connection, roots, &batch.pending_unlinks, &path_index).await?;
    Ok(batch)
}

async fn reclaim_record(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    record: &IndexedAttachment,
    batch: &mut GcBatch,
    path_index: &mut PathReferenceIndex,
) -> Result<(), String> {
    if !is_valid_cas_hash(&record.hash) {
        clear_record_unlink_debts(connection, roots, record).await?;
        return Ok(());
    }
    if path_index.has_live_reference(&record.hash) {
        clear_record_unlink_debts(connection, roots, record).await?;
        batch.report.retained += 1;
        return Ok(());
    }
    let paths = validate_record_paths(record, roots);
    if !paths.safe_for_delete {
        clear_record_unlink_debts(connection, roots, record).await?;
        batch.report.deferred += 1;
        return Ok(());
    }
    if !delete_index_if_orphaned(connection, &record.hash).await? {
        clear_record_unlink_debts(connection, roots, record).await?;
        return Ok(());
    }
    path_index.remove_record(roots, record);
    batch.report.reclaimed += 1;
    queue_unlinks_after_cas(paths, batch);
    Ok(())
}

fn queue_unlinks_after_cas(paths: RecordPaths, batch: &mut GcBatch) {
    for (root_kind, path) in [
        (RootKind::Attachment, paths.internal),
        (RootKind::Thumbnail, paths.thumbnail),
    ] {
        let Some(path) = path else { continue };
        batch
            .pending_unlinks
            .push(PendingUnlink { root_kind, path });
    }
}

async fn clear_record_unlink_debts(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    record: &IndexedAttachment,
) -> Result<(), String> {
    for (kind, root, raw_path) in [
        (
            RootKind::Attachment,
            &roots.attachments,
            Some(record.internal_path.as_str()),
        ),
        (
            RootKind::Thumbnail,
            &roots.thumbnails,
            record.thumbnail_path.as_deref(),
        ),
    ] {
        let Some(raw_path) = raw_path.filter(|path| !path.is_empty()) else {
            continue;
        };
        let canonical_path = match validate_reference_path(root, raw_path)
            .or_else(|_| resolve_reference_target_through_symlinks(root, raw_path))
        {
            Ok((path, _)) => path,
            Err(_) => {
                let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
                let candidate = std::path::Path::new(clean_path);
                // A reference whose existing target is provably outside this
                // managed root cannot keep a managed file alive. This covers
                // an in-root symlink pointing outside the root; unresolved
                // aliases remain fail-closed below.
                if let (Ok(canonical_root), Ok(canonical_target)) = (
                    canonical_managed_root(root),
                    std::fs::canonicalize(candidate),
                ) {
                    if !canonical_target.starts_with(&canonical_root) {
                        continue;
                    }
                }
                let unresolved_symlink = std::fs::symlink_metadata(candidate)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink());
                if path_may_be_inside_managed_root(root, raw_path) || unresolved_symlink {
                    return Err("附件路径存在无法安全规范化的活引用，拒绝物理删除".to_string());
                }
                continue;
            }
        };
        let Ok(relative) = relative_path_for_validated_root(root, &canonical_path) else {
            return Err("附件路径存在无法安全规范化的活引用，拒绝物理删除".to_string());
        };
        clear_unlink_debt(connection, root_kind_key(kind), &relative).await?;
    }
    Ok(())
}

async fn enqueue_pending_unlinks(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    pending: &[PendingUnlink],
    path_index: &PathReferenceIndex,
) -> Result<(), String> {
    for pending in pending {
        let root = match pending.root_kind {
            RootKind::Attachment => &roots.attachments,
            RootKind::Thumbnail => &roots.thumbnails,
            RootKind::MultimodalCache => &roots.multimodal_cache,
        };
        match path_index.path_reference_state(root, &pending.path)? {
            PathReferenceState::Referenced => {}
            PathReferenceState::Unreferenced => {
                let relative = relative_path_for_root(root, &pending.path)?;
                enqueue_unlink(connection, root_kind_key(pending.root_kind), &relative).await?;
            }
            PathReferenceState::Uncertain => {
                return Err(
                    "附件路径存在无法安全规范化的活引用，拒绝记录物理 unlink 债务".to_string(),
                );
            }
        }
    }
    Ok(())
}

pub(super) async fn commit_gc_batch(
    connection: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    roots: &ManagedAttachmentRoots,
    result: Result<GcBatch, String>,
) -> Result<AttachmentGcReport, String> {
    commit_gc_batch_inner(connection, roots, result, false).await
}

pub(super) async fn commit_gc_batch_inner(
    connection: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    roots: &ManagedAttachmentRoots,
    result: Result<GcBatch, String>,
    inject_commit_failure: bool,
) -> Result<AttachmentGcReport, String> {
    match result {
        Ok(batch) => {
            if let Err(error) = persist_gc_cursors(connection, &batch).await {
                let _ = sqlx::query("ROLLBACK").execute(&mut **connection).await;
                return Err(error);
            }
            if inject_commit_failure {
                let _ = sqlx::query("ROLLBACK").execute(&mut **connection).await;
                return Err("测试注入附件 GC 提交失败".to_string());
            }
            let commit = sqlx::query("COMMIT")
                .execute(&mut **connection)
                .await
                .map_err(|error| format!("提交附件 GC CAS 事务失败: {error}"));
            if let Err(error) = commit {
                let _ = sqlx::query("ROLLBACK").execute(&mut **connection).await;
                return Err(error);
            }
            finish_gc_batch(&mut *connection, roots, batch).await
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut **connection).await;
            Err(error)
        }
    }
}

async fn finish_gc_batch(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    batch: GcBatch,
) -> Result<AttachmentGcReport, String> {
    if batch.reset_cycle {
        let mut report = batch.report;
        report.has_more = true;
        return Ok(report);
    }
    let page_exhausted = batch.page_exhausted;
    let first_page = batch.first_page;
    let cycle_start = batch
        .cycle_state
        .as_ref()
        .map(|state| state.cycle_start)
        .ok_or_else(|| "附件 GC 周期状态缺失，拒绝物理 unlink".to_string())?;
    let mut report = batch.report;
    let unlink_report = if page_exhausted {
        retry_unlink_outbox_before(connection, roots, (!first_page).then_some(cycle_start)).await?
    } else {
        Default::default()
    };
    report.ghost_files += unlink_report.removed;
    report.deferred += unlink_report.deferred;
    report.has_more |= unlink_report.has_more;
    if page_exhausted && !first_page && has_unlink_debts(connection).await? {
        report.has_more = true;
    }
    for (root, kind) in [
        (&roots.attachments, RootKind::Attachment),
        (&roots.thumbnails, RootKind::Thumbnail),
        (&roots.multimodal_cache, RootKind::MultimodalCache),
    ] {
        let sweep = super::paths::sweep_root_after_commit(connection, roots, root, kind).await?;
        report.ghost_files += sweep.removed;
        report.has_more |= sweep.has_more;
    }
    Ok(report)
}

async fn persist_gc_cursors(
    connection: &mut sqlx::pool::PoolConnection<sqlx::Sqlite>,
    batch: &GcBatch,
) -> Result<(), String> {
    let attachment_cursor = if batch.reset_cycle || batch.page_exhausted {
        ""
    } else {
        batch.next_cursor.as_deref().unwrap_or_default()
    };
    write_gc_cursor(connection, ATTACHMENT_GC_CURSOR_KEY, attachment_cursor).await?;
    let cycle_state = if batch.reset_cycle || batch.page_exhausted {
        String::new()
    } else {
        serde_json::to_string(
            batch
                .cycle_state
                .as_ref()
                .ok_or_else(|| "附件 GC 周期状态缺失，拒绝保存游标".to_string())?,
        )
        .map_err(|error| format!("序列化附件 GC 周期状态失败: {error}"))?
    };
    write_gc_cursor(connection, ATTACHMENT_GC_CYCLE_KEY, &cycle_state).await?;
    let relation_cursor = if batch.relation_exhausted {
        String::new()
    } else {
        batch
            .relation_cursor
            .map(|cursor| cursor.to_string())
            .unwrap_or_default()
    };
    write_gc_cursor(connection, RELATION_GC_CURSOR_KEY, &relation_cursor).await
}
