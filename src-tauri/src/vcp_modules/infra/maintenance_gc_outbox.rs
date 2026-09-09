use super::db::{
    clear_unlink_debt, load_unlink_debts, load_unlink_live_reference_hashes, read_gc_cursor,
    write_gc_cursor, PathReferenceIndex, PathReferenceState, UnlinkDebt, UnlinkDebtCursor,
    UNLINK_OUTBOX_CURSOR_KEY,
};
use super::paths::{
    remove_indexed_path, resolve_relative_indexed_path, root_kind_from_key, ManagedAttachmentRoots,
    ManagedPathState, RootKind,
};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct UnlinkReport {
    pub(super) removed: usize,
    pub(super) deferred: usize,
    pub(super) has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnlinkAttempt {
    Removed,
    Cleared,
    Deferred,
}

/// Retry a bounded set of persisted unlink debts. Each row is checked and
/// cleared in a transaction that also serializes ordinary attachment writes;
/// a failed unlink rolls that transaction back and leaves the debt intact.
pub(super) async fn retry_unlink_outbox(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
) -> Result<UnlinkReport, String> {
    retry_unlink_outbox_before(connection, roots, None).await
}

/// Retry only debts created before an attachment GC cycle boundary. Debts
/// created by the current cycle are intentionally left for the next complete
/// attachment scan, so a reference encountered earlier in the sort order can
/// still clear them before physical unlinking is attempted.
pub(super) async fn retry_unlink_outbox_before(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    created_before: Option<i64>,
) -> Result<UnlinkReport, String> {
    let persisted_cursor = read_gc_cursor(connection, UNLINK_OUTBOX_CURSOR_KEY).await?;
    let cursor = serde_json::from_str::<UnlinkDebtCursor>(&persisted_cursor).ok();
    let debt_page = load_unlink_debts(connection, cursor.as_ref(), created_before).await?;
    let path_keys = debt_page
        .debts
        .iter()
        .filter_map(|debt| {
            let kind = root_kind_from_key(&debt.root_kind)?;
            let root = match kind {
                RootKind::Attachment => &roots.attachments,
                RootKind::Thumbnail => &roots.thumbnails,
                RootKind::MultimodalCache => &roots.multimodal_cache,
            };
            resolve_relative_indexed_path(root, &debt.relative_path)
                .ok()
                .map(|(path, _)| (kind, path))
        })
        .collect::<Vec<_>>();
    let path_index = PathReferenceIndex::load_for_paths(connection, roots, &path_keys, &[]).await?;
    retry_unlink_outbox_page(connection, roots, debt_page, &path_index).await
}

pub(super) async fn retry_unlink_outbox_with_index(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    path_index: &PathReferenceIndex,
) -> Result<UnlinkReport, String> {
    let persisted_cursor = read_gc_cursor(connection, UNLINK_OUTBOX_CURSOR_KEY).await?;
    let cursor = serde_json::from_str::<UnlinkDebtCursor>(&persisted_cursor).ok();
    let debt_page = load_unlink_debts(connection, cursor.as_ref(), None).await?;
    retry_unlink_outbox_page(connection, roots, debt_page, path_index).await
}

async fn retry_unlink_outbox_page(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    debt_page: super::db::UnlinkDebtPage,
    path_index: &PathReferenceIndex,
) -> Result<UnlinkReport, String> {
    let mut report = UnlinkReport {
        has_more: debt_page.has_more,
        ..UnlinkReport::default()
    };
    for debt in debt_page.debts {
        match retry_unlink_debt(connection, roots, path_index, &debt).await? {
            UnlinkAttempt::Removed => report.removed += 1,
            UnlinkAttempt::Cleared => {}
            UnlinkAttempt::Deferred => report.deferred += 1,
        }
    }
    let next_cursor = if debt_page.has_more {
        debt_page.next_cursor
    } else {
        None
    };
    let serialized_cursor = next_cursor
        .map(|cursor| serde_json::to_string(&cursor).unwrap_or_default())
        .unwrap_or_default();
    write_gc_cursor(connection, UNLINK_OUTBOX_CURSOR_KEY, &serialized_cursor).await?;
    Ok(report)
}

async fn retry_unlink_debt(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    path_index: &PathReferenceIndex,
    debt: &UnlinkDebt,
) -> Result<UnlinkAttempt, String> {
    let Some((kind, root, path, state)) = resolve_debt_path(roots, debt) else {
        return Ok(UnlinkAttempt::Deferred);
    };
    begin_unlink_transaction(connection).await?;
    // The preloaded page index is only a fast first check.  A writer on a
    // different pool connection may have committed a new reference after
    // that snapshot, so every debt gets a fresh check after BEGIN IMMEDIATE
    // on this very transaction's connection.
    let snapshot_uncertain = matches!(
        path_index.path_reference_state(root, &path),
        Ok(PathReferenceState::Uncertain) | Err(_)
    );

    let Some(live_hashes) = load_live_hashes_or_defer(connection, debt).await? else {
        return Ok(UnlinkAttempt::Deferred);
    };
    let (final_state, has_live_hash_reference) =
        load_final_reference_state(connection, roots, kind, root, &path, &live_hashes).await;
    if has_live_hash_reference {
        return clear_referenced_debt(connection, debt).await;
    }
    if matches!(final_state, Ok(PathReferenceState::Referenced)) {
        return clear_referenced_debt(connection, debt).await;
    }
    if !is_safe_to_unlink(final_state, snapshot_uncertain) {
        rollback_unlink_transaction(connection).await;
        return Ok(UnlinkAttempt::Deferred);
    }
    unlink_unreferenced_debt(connection, root, path, state, debt).await
}

fn resolve_debt_path<'a>(
    roots: &'a ManagedAttachmentRoots,
    debt: &UnlinkDebt,
) -> Option<(
    RootKind,
    &'a std::path::Path,
    std::path::PathBuf,
    ManagedPathState,
)> {
    let kind = root_kind_from_key(&debt.root_kind)?;
    let root = match kind {
        RootKind::Attachment => &roots.attachments,
        RootKind::Thumbnail => &roots.thumbnails,
        RootKind::MultimodalCache => &roots.multimodal_cache,
    };
    let (path, state) = resolve_relative_indexed_path(root, &debt.relative_path).ok()?;
    Some((kind, root, path, state))
}

async fn begin_unlink_transaction(connection: &mut sqlx::SqliteConnection) -> Result<(), String> {
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await
        .map(|_| ())
        .map_err(|error| format!("开启附件 unlink 重试事务失败: {error}"))
}

async fn load_live_hashes_or_defer(
    connection: &mut sqlx::SqliteConnection,
    debt: &UnlinkDebt,
) -> Result<Option<Vec<String>>, String> {
    let live_hashes =
        load_unlink_live_reference_hashes(connection, &debt.root_kind, &debt.relative_path).await?;
    if live_hashes.is_none() {
        rollback_unlink_transaction(connection).await;
    }
    Ok(live_hashes)
}

async fn load_final_reference_state(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    kind: RootKind,
    root: &std::path::Path,
    path: &std::path::Path,
    live_hashes: &[String],
) -> (Result<PathReferenceState, String>, bool) {
    let final_index = PathReferenceIndex::load_for_paths(
        connection,
        roots,
        &[(kind, path.to_path_buf())],
        live_hashes,
    )
    .await;
    match final_index {
        Ok(index) => {
            let state = index.path_reference_state(root, path);
            let has_live_hash_reference = live_hashes
                .iter()
                .any(|hash| index.has_live_reference_at_path(hash, root, path));
            (state, has_live_hash_reference)
        }
        Err(error) => (Err(error), false),
    }
}

fn is_safe_to_unlink(
    final_state: Result<PathReferenceState, String>,
    snapshot_uncertain: bool,
) -> bool {
    matches!(
        (final_state, snapshot_uncertain),
        (Ok(PathReferenceState::Unreferenced), false)
    )
}

async fn rollback_unlink_transaction(connection: &mut sqlx::SqliteConnection) {
    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
}

async fn clear_referenced_debt(
    connection: &mut sqlx::SqliteConnection,
    debt: &UnlinkDebt,
) -> Result<UnlinkAttempt, String> {
    if let Err(error) = clear_unlink_debt(connection, &debt.root_kind, &debt.relative_path).await {
        let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        return Err(error);
    }
    if let Err(error) = sqlx::query("COMMIT").execute(&mut *connection).await {
        let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        return Err(format!("提交过期附件 unlink 债务清理失败: {error}"));
    }
    Ok(UnlinkAttempt::Cleared)
}

async fn unlink_unreferenced_debt(
    connection: &mut sqlx::SqliteConnection,
    root: &std::path::Path,
    path: std::path::PathBuf,
    state: ManagedPathState,
    debt: &UnlinkDebt,
) -> Result<UnlinkAttempt, String> {
    let deleted = match remove_indexed_path(root, &path.to_string_lossy()).await {
        Ok(deleted) => deleted,
        Err(_) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            return Ok(UnlinkAttempt::Deferred);
        }
    };
    if clear_unlink_debt(connection, &debt.root_kind, &debt.relative_path)
        .await
        .is_err()
    {
        let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        return Ok(UnlinkAttempt::Deferred);
    }
    if let Err(error) = sqlx::query("COMMIT").execute(&mut *connection).await {
        let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        return Err(format!("提交附件 unlink 重试事务失败: {error}"));
    }
    Ok(if deleted && state == ManagedPathState::Present {
        UnlinkAttempt::Removed
    } else {
        UnlinkAttempt::Cleared
    })
}
