use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::{ManagedAttachmentRoots, RootKind};
#[path = "maintenance_gc_db_debts.rs"]
mod debts;
#[path = "maintenance_gc_db_relations.rs"]
mod relations;
pub(super) use super::reference::{IndexedAttachment, PathReferenceIndex, PathReferenceState};
pub(super) use debts::load_unlink_debts;
pub(super) use relations::scrub_deleted_attachment_links;

pub(super) const GC_PAGE_SIZE: i64 = 256;
pub(super) const ATTACHMENT_GC_CURSOR_KEY: &str = "maintenance.attachment_gc.cursor";
pub(super) const ATTACHMENT_GC_CYCLE_KEY: &str = "maintenance.attachment_gc.cycle_start";
pub(super) const RELATION_GC_CURSOR_KEY: &str = "maintenance.attachment_relation_gc.cursor";
pub(super) const ATTACHMENT_SCAN_CURSOR_KEY: &str =
    "maintenance.attachment_gc.scan_cursor.attachment";
pub(super) const THUMBNAIL_SCAN_CURSOR_KEY: &str =
    "maintenance.attachment_gc.scan_cursor.thumbnail";
pub(super) const MULTIMODAL_CACHE_SCAN_CURSOR_KEY: &str =
    "maintenance.attachment_gc.scan_cursor.multimodal_cache";
pub(super) const UNLINK_OUTBOX_PAGE_SIZE: i64 = 256;
pub(super) const UNLINK_OUTBOX_CURSOR_KEY: &str = "maintenance.attachment_gc.unlink_cursor";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AttachmentGcCycleState {
    pub(super) token: String,
    pub(super) cycle_start: i64,
    pub(super) next_cursor: String,
}

#[derive(Debug, Default)]
pub(super) struct AttachmentPage {
    pub(super) records: Vec<IndexedAttachment>,
    pub(super) has_more: bool,
}

#[derive(Debug, Clone)]
pub(super) struct UnlinkDebt {
    pub(super) root_kind: String,
    pub(super) relative_path: String,
    pub(super) created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct UnlinkDebtCursor {
    pub(super) created_at: i64,
    pub(super) root_kind: String,
    pub(super) relative_path: String,
}

#[derive(Debug, Default)]
pub(super) struct UnlinkDebtPage {
    pub(super) debts: Vec<UnlinkDebt>,
    pub(super) has_more: bool,
    pub(super) next_cursor: Option<UnlinkDebtCursor>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct RelationCleanupPage {
    pub(super) next_cursor: Option<i64>,
    pub(super) exhausted: bool,
}

/// 用 hash 游标读取一页索引；每次维护最多处理一个固定页，避免全表快照。
pub(super) async fn load_attachment_page(
    connection: &mut sqlx::SqliteConnection,
    cursor: &str,
) -> Result<AttachmentPage, String> {
    sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT hash, internal_path, thumbnail_path
         FROM attachments WHERE hash > ? ORDER BY hash ASC LIMIT ?",
    )
    .bind(cursor)
    .bind(GC_PAGE_SIZE + 1)
    .fetch_all(&mut *connection)
    .await
    .map(|mut rows| {
        let has_more = rows.len() > GC_PAGE_SIZE as usize;
        rows.truncate(GC_PAGE_SIZE as usize);
        AttachmentPage {
            has_more,
            records: rows
                .into_iter()
                .map(|(hash, internal_path, thumbnail_path)| IndexedAttachment {
                    hash,
                    internal_path,
                    thumbnail_path,
                })
                .collect(),
        }
    })
    .map_err(|error| format!("读取附件 GC 索引分页失败: {error}"))
}

pub(super) async fn has_live_reference(
    connection: &mut sqlx::SqliteConnection,
    hash: &str,
) -> Result<bool, String> {
    let live: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
          SELECT 1 FROM message_attachments ma
          JOIN messages m ON m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
             AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
          JOIN topics t ON t.owner_type = m.owner_type AND t.owner_id = m.owner_id
             AND t.topic_id = m.topic_id
          WHERE LOWER(ma.hash) = LOWER(?) AND ma.deleted_at IS NULL AND m.deleted_at IS NULL
            AND t.deleted_at IS NULL
            AND (t.owner_type NOT IN ('agent', 'group')
              OR (t.owner_type = 'agent' AND EXISTS (
                SELECT 1 FROM agents a WHERE a.agent_id = t.owner_id
                  AND a.deleted_at IS NULL
              ))
              OR (t.owner_type = 'group' AND EXISTS (
                SELECT 1 FROM groups g WHERE g.group_id = t.owner_id
                  AND g.deleted_at IS NULL
              )))
        )",
    )
    .bind(hash)
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| format!("再次检查附件有效引用失败: {error}"))?;
    Ok(live != 0)
}

/// 在当前写事务内做最后一次引用 CAS，并且只允许精确删除一条索引。
pub(super) async fn delete_index_if_orphaned(
    connection: &mut sqlx::SqliteConnection,
    hash: &str,
) -> Result<bool, String> {
    let deleted = sqlx::query(
        "DELETE FROM attachments
         WHERE hash = ?
           AND NOT EXISTS (
             SELECT 1 FROM message_attachments ma
             JOIN messages m ON m.owner_type = ma.owner_type
                AND m.owner_id = ma.owner_id AND m.topic_id = ma.topic_id
                AND m.msg_id = ma.msg_id
             JOIN topics t ON t.owner_type = m.owner_type
                AND t.owner_id = m.owner_id AND t.topic_id = m.topic_id
             WHERE LOWER(ma.hash) = LOWER(?) AND ma.deleted_at IS NULL
               AND m.deleted_at IS NULL AND t.deleted_at IS NULL
               AND (t.owner_type NOT IN ('agent', 'group')
                 OR (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a WHERE a.agent_id = t.owner_id
                     AND a.deleted_at IS NULL
                 ))
                 OR (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g WHERE g.group_id = t.owner_id
                     AND g.deleted_at IS NULL
                 )))
           )",
    )
    .bind(hash)
    .bind(hash)
    .execute(&mut *connection)
    .await
    .map_err(|error| format!("删除附件索引 {hash} 失败: {error}"))?;
    Ok(deleted.rows_affected() == 1)
}

/// 删除索引后，在同一事务中确认物理路径仍无其他附件索引引用。
pub(super) async fn path_reference_count(
    connection: &mut sqlx::SqliteConnection,
    root: &Path,
    path: &Path,
) -> Result<i64, String> {
    let attachments_root = root.to_path_buf();
    let roots =
        ManagedAttachmentRoots::new(attachments_root, root.to_path_buf(), root.to_path_buf());
    let index = PathReferenceIndex::load_for_paths(
        connection,
        &roots,
        &[(RootKind::Attachment, path.to_path_buf())],
        &[],
    )
    .await?;
    match index.path_reference_state(root, path)? {
        PathReferenceState::Referenced => Ok(1),
        PathReferenceState::Unreferenced => Ok(0),
        PathReferenceState::Uncertain => {
            Err("附件路径存在无法安全规范化的活引用，拒绝物理删除".to_string())
        }
    }
}

/// 目录扫描删除前以精确 DB 路径为准，避免只依赖文件名猜测。
pub(super) async fn path_is_indexed(
    connection: &mut sqlx::SqliteConnection,
    root: &Path,
    path: &Path,
) -> Result<bool, String> {
    Ok(path_reference_count(connection, root, path).await? > 0)
}

pub(super) async fn has_indexed_hash(
    connection: &mut sqlx::SqliteConnection,
    hash: &str,
) -> Result<bool, String> {
    let exists: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachments WHERE LOWER(hash) = LOWER(?))")
            .bind(hash)
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| format!("检查附件索引 hash 失败: {error}"))?;
    Ok(exists != 0)
}

pub(super) async fn enqueue_unlink(
    connection: &mut sqlx::SqliteConnection,
    root_kind: &str,
    relative_path: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO attachment_gc_unlink_outbox (root_kind, relative_path, created_at)
         VALUES (?, ?, ?)
         ON CONFLICT(root_kind, relative_path) DO NOTHING",
    )
    .bind(root_kind)
    .bind(relative_path)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(|error| format!("记录附件物理 unlink 债务失败: {error}"))
}

/// Persist a bounded witness that a live attachment hash resolved to this
/// exact unlink debt. This is used only for paths that the write-side strict
/// validator cannot classify (for example an external or ancestor symlink).
pub(super) async fn record_unlink_live_reference(
    connection: &mut sqlx::SqliteConnection,
    root_kind: &str,
    relative_path: &str,
    hash: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT OR IGNORE INTO attachment_gc_unlink_live_references
            (root_kind, relative_path, hash)
         SELECT ?, ?, ?
         WHERE EXISTS (
            SELECT 1 FROM attachment_gc_unlink_outbox
            WHERE root_kind = ? AND relative_path = ?
         )",
    )
    .bind(root_kind)
    .bind(relative_path)
    .bind(hash)
    .bind(root_kind)
    .bind(relative_path)
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(|error| format!("记录附件 unlink 活引用见证失败: {error}"))
}

pub(super) fn record_unlink_live_reference_rusqlite(
    transaction: &rusqlite::Transaction<'_>,
    root_kind: &str,
    relative_path: &str,
    hash: &str,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO attachment_gc_unlink_live_references
            (root_kind, relative_path, hash)
         SELECT ?1, ?2, ?3
         WHERE EXISTS (
            SELECT 1 FROM attachment_gc_unlink_outbox
            WHERE root_kind = ?1 AND relative_path = ?2
         )",
        rusqlite::params![root_kind, relative_path, hash],
    )?;
    Ok(())
}

/// Load at most one page of witnesses for a single debt. More than the page
/// budget is an uncertainty signal: do not truncate live hashes and proceed.
pub(super) async fn load_unlink_live_reference_hashes(
    connection: &mut sqlx::SqliteConnection,
    root_kind: &str,
    relative_path: &str,
) -> Result<Option<Vec<String>>, String> {
    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT hash FROM attachment_gc_unlink_live_references
         WHERE root_kind = ? AND relative_path = ?
         ORDER BY hash COLLATE NOCASE
         LIMIT ?",
    )
    .bind(root_kind)
    .bind(relative_path)
    .bind(UNLINK_OUTBOX_PAGE_SIZE + 1)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| format!("读取附件 unlink 活引用见证失败: {error}"))?;
    if hashes.len() > UNLINK_OUTBOX_PAGE_SIZE as usize {
        return Ok(None);
    }
    Ok(Some(hashes))
}

pub(super) async fn clear_unlink_debt(
    connection: &mut sqlx::SqliteConnection,
    root_kind: &str,
    relative_path: &str,
) -> Result<bool, String> {
    let deleted = sqlx::query(
        "DELETE FROM attachment_gc_unlink_outbox
         WHERE root_kind = ? AND relative_path = ?",
    )
    .bind(root_kind)
    .bind(relative_path)
    .execute(&mut *connection)
    .await
    .map_err(|error| format!("清除附件物理 unlink 债务失败: {error}"))?;
    sqlx::query(
        "DELETE FROM attachment_gc_unlink_live_references
         WHERE root_kind = ? AND relative_path = ?",
    )
    .bind(root_kind)
    .bind(relative_path)
    .execute(&mut *connection)
    .await
    .map_err(|error| format!("清除附件 unlink 活引用见证失败: {error}"))?;
    Ok(deleted.rows_affected() == 1)
}

pub(super) async fn has_unlink_debts(
    connection: &mut sqlx::SqliteConnection,
) -> Result<bool, String> {
    let exists: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM attachment_gc_unlink_outbox)")
            .fetch_one(&mut *connection)
            .await
            .map_err(|error| format!("检查附件物理 unlink 债务失败: {error}"))?;
    Ok(exists != 0)
}

pub(super) async fn read_gc_cursor(
    connection: &mut sqlx::SqliteConnection,
    key: &str,
) -> Result<String, String> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(&mut *connection)
        .await
        .map(|value: Option<String>| value.unwrap_or_default())
        .map_err(|error| format!("读取附件 GC 游标失败: {error}"))
}

pub(super) async fn write_gc_cursor(
    connection: &mut sqlx::SqliteConnection,
    key: &str,
    value: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO settings(key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value,
             updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(crate::vcp_modules::infra::utils::now_millis())
    .execute(&mut *connection)
    .await
    .map(|_| ())
    .map_err(|error| format!("保存附件 GC 游标失败: {error}"))
}
