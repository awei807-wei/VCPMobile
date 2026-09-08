use super::db::{
    clear_unlink_debt, record_unlink_live_reference, record_unlink_live_reference_rusqlite,
};
use super::paths::{
    live_reference_unlink_relative_path, live_reference_unlink_relative_path_through_symlinks,
    root_kind_key, ManagedAttachmentRoots, RootKind,
};

/// 在附件/关系写事务内清除同一物理附件的旧 unlink 债务。
///
/// GC 页之间允许生产写入继续进行；若低 hash 的附件在前一页被回收后又
/// 被重新建立关系，单靠下一页快照无法看到它。写入口在自己的 DB 事务中
/// 调用此函数，按当前 live relation 查询 canonical 物理路径并原子删除
/// 对应债务。路径无法证明属于受管 root 时保留债务，物理清理由后续 GC
/// 的引用终检决定。
pub(crate) async fn clear_live_attachment_unlink_debts(
    connection: &mut sqlx::SqliteConnection,
    hash: &str,
    roots: &ManagedAttachmentRoots,
) -> Result<(), String> {
    let Some(paths) = load_live_attachment_paths(connection, hash).await? else {
        return Ok(());
    };
    for (internal_path, thumbnail_path) in &paths {
        clear_live_attachment_path(connection, roots, hash, RootKind::Attachment, internal_path)
            .await?;
        if let Some(thumbnail_path) = thumbnail_path.as_deref() {
            clear_live_attachment_path(
                connection,
                roots,
                hash,
                RootKind::Thumbnail,
                thumbnail_path,
            )
            .await?;
        }
    }
    Ok(())
}

async fn load_live_attachment_paths(
    connection: &mut sqlx::SqliteConnection,
    hash: &str,
) -> Result<Option<Vec<(String, Option<String>)>>, String> {
    let outbox_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'attachment_gc_unlink_outbox'
        )",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|error| format!("检查附件物理 unlink 债务表失败: {error}"))?;
    if outbox_exists == 0 {
        return Ok(None);
    }
    let paths: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT DISTINCT a.internal_path, a.thumbnail_path
         FROM attachments a
         WHERE a.hash COLLATE NOCASE = ?
           AND EXISTS (
             SELECT 1 FROM message_attachments ma
             JOIN messages m ON m.owner_type = ma.owner_type
                AND m.owner_id = ma.owner_id AND m.topic_id = ma.topic_id
                AND m.msg_id = ma.msg_id
             JOIN topics t ON t.owner_type = m.owner_type
                AND t.owner_id = m.owner_id AND t.topic_id = m.topic_id
             WHERE ma.hash COLLATE NOCASE = a.hash COLLATE NOCASE
               AND ma.deleted_at IS NULL AND m.deleted_at IS NULL
               AND t.deleted_at IS NULL
               AND (t.owner_type NOT IN ('agent', 'group')
                 OR (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a2
                   WHERE a2.agent_id = t.owner_id AND a2.deleted_at IS NULL
                 ))
                 OR (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g
                   WHERE g.group_id = t.owner_id AND g.deleted_at IS NULL
                 )))
           )",
    )
    .bind(hash)
    .fetch_all(&mut *connection)
    .await
    .map_err(|error| format!("读取活附件物理路径失败: {error}"))?;
    Ok(Some(paths))
}

async fn clear_live_attachment_path(
    connection: &mut sqlx::SqliteConnection,
    roots: &ManagedAttachmentRoots,
    hash: &str,
    kind: RootKind,
    raw_path: &str,
) -> Result<(), String> {
    let root = match kind {
        RootKind::Attachment => &roots.attachments,
        RootKind::Thumbnail => &roots.thumbnails,
        RootKind::MultimodalCache => return Ok(()),
    };
    if let Some(relative) = live_reference_unlink_relative_path(root, raw_path) {
        clear_unlink_debt(connection, root_kind_key(kind), &relative).await?;
    } else if let Some(relative) =
        live_reference_unlink_relative_path_through_symlinks(root, raw_path)
    {
        record_unlink_live_reference(connection, root_kind_key(kind), &relative, hash).await?;
    }
    Ok(())
}

/// rusqlite 对应的同步写队列版本；调用方必须已持有附件 GC 读闸门，并
/// 在与附件索引、消息关系相同的事务中调用。
pub(crate) fn clear_live_attachment_unlink_debts_rusqlite(
    transaction: &rusqlite::Transaction<'_>,
    hash: &str,
    roots: &ManagedAttachmentRoots,
) -> rusqlite::Result<()> {
    let Some(paths) = load_live_attachment_paths_rusqlite(transaction, hash)? else {
        return Ok(());
    };
    for (internal_path, thumbnail_path) in &paths {
        clear_live_attachment_path_rusqlite(
            transaction,
            roots,
            hash,
            RootKind::Attachment,
            Some(internal_path),
        )?;
        clear_live_attachment_path_rusqlite(
            transaction,
            roots,
            hash,
            RootKind::Thumbnail,
            thumbnail_path.as_ref(),
        )?;
    }
    Ok(())
}

fn load_live_attachment_paths_rusqlite(
    transaction: &rusqlite::Transaction<'_>,
    hash: &str,
) -> rusqlite::Result<Option<Vec<(String, Option<String>)>>> {
    let outbox_exists: i64 = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
            WHERE type = 'table' AND name = 'attachment_gc_unlink_outbox'
        )",
        [],
        |row| row.get(0),
    )?;
    if outbox_exists == 0 {
        return Ok(None);
    }
    let mut statement = transaction.prepare(
        "SELECT DISTINCT a.internal_path, a.thumbnail_path
         FROM attachments a
         WHERE a.hash COLLATE NOCASE = ?
           AND EXISTS (
             SELECT 1 FROM message_attachments ma
             JOIN messages m ON m.owner_type = ma.owner_type
                AND m.owner_id = ma.owner_id AND m.topic_id = ma.topic_id
                AND m.msg_id = ma.msg_id
             JOIN topics t ON t.owner_type = m.owner_type
                AND t.owner_id = m.owner_id AND t.topic_id = m.topic_id
             WHERE ma.hash COLLATE NOCASE = a.hash COLLATE NOCASE
               AND ma.deleted_at IS NULL AND m.deleted_at IS NULL
               AND t.deleted_at IS NULL
               AND (t.owner_type NOT IN ('agent', 'group')
                 OR (t.owner_type = 'agent' AND EXISTS (
                   SELECT 1 FROM agents a2
                   WHERE a2.agent_id = t.owner_id AND a2.deleted_at IS NULL
                 ))
                 OR (t.owner_type = 'group' AND EXISTS (
                   SELECT 1 FROM groups g
                   WHERE g.group_id = t.owner_id AND g.deleted_at IS NULL
                 )))
           )",
    )?;
    let paths = statement
        .query_map(rusqlite::params![hash], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(paths))
}

fn clear_live_attachment_path_rusqlite(
    transaction: &rusqlite::Transaction<'_>,
    roots: &ManagedAttachmentRoots,
    hash: &str,
    kind: RootKind,
    raw_path: Option<&String>,
) -> rusqlite::Result<()> {
    let Some(raw_path) = raw_path else {
        return Ok(());
    };
    let root = match kind {
        RootKind::Attachment => &roots.attachments,
        RootKind::Thumbnail => &roots.thumbnails,
        RootKind::MultimodalCache => return Ok(()),
    };
    if let Some(relative) = live_reference_unlink_relative_path(root, raw_path) {
        transaction.execute(
            "DELETE FROM attachment_gc_unlink_outbox
                 WHERE root_kind = ? AND relative_path = ?",
            rusqlite::params![root_kind_key(kind), relative],
        )?;
    } else if let Some(relative) =
        live_reference_unlink_relative_path_through_symlinks(root, raw_path)
    {
        record_unlink_live_reference_rusqlite(transaction, root_kind_key(kind), &relative, hash)?;
    }
    Ok(())
}
