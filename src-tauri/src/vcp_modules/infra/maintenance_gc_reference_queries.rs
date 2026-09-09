fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) async fn query_attachment_rows_by_hash(
    connection: &mut sqlx::SqliteConnection,
    hashes: &[String],
) -> Result<Vec<(String, String, Option<String>)>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT hash, internal_path, thumbnail_path
         FROM attachments WHERE hash COLLATE NOCASE IN ({})",
        placeholders(hashes.len())
    );
    let mut query = sqlx::query_as::<_, (String, String, Option<String>)>(&sql);
    for hash in hashes {
        query = query.bind(hash);
    }
    query
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| format!("读取候选附件索引引用失败: {error}"))
}

pub(super) async fn query_live_hashes(
    connection: &mut sqlx::SqliteConnection,
    hashes: &[String],
) -> Result<Vec<String>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT DISTINCT LOWER(ma.hash)
         FROM message_attachments ma
         JOIN messages m ON m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
            AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
         JOIN topics t ON t.owner_type = m.owner_type AND t.owner_id = m.owner_id
            AND t.topic_id = m.topic_id
         WHERE ma.hash COLLATE NOCASE IN ({})
           AND ma.deleted_at IS NULL AND m.deleted_at IS NULL AND t.deleted_at IS NULL
           AND (t.owner_type NOT IN ('agent', 'group')
             OR (t.owner_type = 'agent' AND EXISTS (
               SELECT 1 FROM agents a WHERE a.agent_id = t.owner_id
                 AND a.deleted_at IS NULL
             ))
             OR (t.owner_type = 'group' AND EXISTS (
               SELECT 1 FROM groups g WHERE g.group_id = t.owner_id
                 AND g.deleted_at IS NULL
             )))",
        placeholders(hashes.len())
    );
    let mut query = sqlx::query_scalar::<_, String>(&sql);
    for hash in hashes {
        query = query.bind(hash);
    }
    query
        .fetch_all(&mut *connection)
        .await
        .map_err(|error| format!("读取候选附件有效引用失败: {error}"))
}

pub(super) async fn query_path_counts(
    connection: &mut sqlx::SqliteConnection,
    column: &str,
    paths: &[String],
) -> Result<Vec<(String, usize)>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {column}, COUNT(*) FROM attachments
         WHERE {column} IN ({}) GROUP BY {column}",
        placeholders(paths.len())
    );
    let mut query = sqlx::query_as::<_, (String, i64)>(&sql);
    for path in paths {
        query = query.bind(path);
    }
    query
        .fetch_all(&mut *connection)
        .await
        .map(|rows| {
            rows.into_iter()
                .filter_map(|(path, count)| usize::try_from(count).ok().map(|count| (path, count)))
                .collect()
        })
        .map_err(|error| format!("读取候选附件路径引用失败: {error}"))
}
