use super::{RelationCleanupPage, GC_PAGE_SIZE};

const DELETED_RELATION_IDS_SQL: &str = "SELECT ma.rowid
         FROM message_attachments AS ma
         WHERE ma.rowid > ? AND (
            ma.deleted_at IS NOT NULL
            OR NOT EXISTS (
              SELECT 1 FROM messages m
              WHERE m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
                AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
            )
            OR NOT EXISTS (
              SELECT 1 FROM topics t
              WHERE t.owner_type = ma.owner_type AND t.owner_id = ma.owner_id
                AND t.topic_id = ma.topic_id
            )
            OR EXISTS (
              SELECT 1 FROM messages m
              WHERE m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
                AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
                AND m.deleted_at IS NOT NULL
            )
            OR EXISTS (
              SELECT 1 FROM topics t
              WHERE t.owner_type = ma.owner_type AND t.owner_id = ma.owner_id
                AND t.topic_id = ma.topic_id AND t.deleted_at IS NOT NULL
            )
            OR EXISTS (
              SELECT 1 FROM topics t JOIN agents a ON a.agent_id = t.owner_id
              WHERE ma.owner_type = 'agent' AND t.owner_type = 'agent'
                AND t.owner_id = ma.owner_id AND t.topic_id = ma.topic_id
                AND a.deleted_at IS NOT NULL
            )
            OR EXISTS (
              SELECT 1 FROM topics t JOIN groups g ON g.group_id = t.owner_id
              WHERE ma.owner_type = 'group' AND t.owner_type = 'group'
                AND t.owner_id = ma.owner_id AND t.topic_id = ma.topic_id
                AND g.deleted_at IS NOT NULL
            )
            OR (ma.owner_type = 'agent' AND NOT EXISTS (
              SELECT 1 FROM agents a
              WHERE a.agent_id = ma.owner_id AND a.deleted_at IS NULL
            ))
            OR (ma.owner_type = 'group' AND NOT EXISTS (
              SELECT 1 FROM groups g
              WHERE g.group_id = ma.owner_id AND g.deleted_at IS NULL
            ))
         )
         ORDER BY ma.rowid ASC LIMIT ?";

pub(crate) async fn scrub_deleted_attachment_links(
    connection: &mut sqlx::SqliteConnection,
    cursor: i64,
) -> Result<RelationCleanupPage, String> {
    let rowids = load_deleted_relation_ids(connection, cursor).await?;
    delete_relation_rows(connection, &rowids).await?;
    Ok(RelationCleanupPage {
        next_cursor: rowids.last().copied(),
        exhausted: rowids.len() < GC_PAGE_SIZE as usize,
    })
}

async fn load_deleted_relation_ids(
    connection: &mut sqlx::SqliteConnection,
    cursor: i64,
) -> Result<Vec<i64>, String> {
    sqlx::query_as::<_, (i64,)>(DELETED_RELATION_IDS_SQL)
        .bind(cursor)
        .bind(GC_PAGE_SIZE)
        .fetch_all(&mut *connection)
        .await
        .map(|rows| rows.into_iter().map(|(rowid,)| rowid).collect())
        .map_err(|error| format!("读取已删除消息附件关系分页失败: {error}"))
}

async fn delete_relation_rows(
    connection: &mut sqlx::SqliteConnection,
    rowids: &[i64],
) -> Result<(), String> {
    for rowid in rowids {
        sqlx::query("DELETE FROM message_attachments WHERE rowid = ?")
            .bind(rowid)
            .execute(&mut *connection)
            .await
            .map_err(|error| format!("删除消息附件关系 {rowid} 失败: {error}"))?;
    }
    Ok(())
}
