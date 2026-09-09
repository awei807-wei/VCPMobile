mod cursor;
pub(crate) mod index_integrity;
mod query;
mod summary;

#[cfg(test)]
mod tests;

use sqlx::{Pool, Row, Sqlite};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchFilter {
    pub query: String,
    pub topic_id: Option<String>,
    pub owner_id: Option<String>,
    pub owner_type: Option<String>,
    #[serde(alias = "agentId")]
    pub speaker_agent_id: Option<String>,
    pub role: Option<String>,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub limit: Option<i64>,
    pub sort: Option<String>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchResult {
    pub msg_id: String,
    pub topic_id: String,
    pub owner_type: String,
    pub owner_id: String,
    pub role: String,
    pub speaker_agent_id: Option<String>,
    pub timestamp: i64,
    pub topic_title: String,
    pub snippet: String,
    pub rank: Option<f64>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchPage {
    pub results: Vec<FtsSearchResult>,
    pub next_cursor: Option<String>,
}

pub(crate) async fn search_messages(
    pool: &Pool<Sqlite>,
    filter: FtsSearchFilter,
) -> Result<FtsSearchPage, String> {
    let plan = query::SearchPlan::from_filter(&filter)?;
    if !plan.has_terms() {
        return Ok(FtsSearchPage {
            results: Vec::new(),
            next_cursor: None,
        });
    }

    let sql = build_sql(&plan, &filter);
    let query = bind_query(sqlx::query(&sql), &plan, &filter);
    let mut rows = query
        .fetch_all(pool)
        .await
        .map_err(|error| format!("全文检索执行失败: {error}"))?;
    let has_more = rows.len() > plan.limit as usize;
    rows.truncate(plan.limit as usize);
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        results.push(decode_result(row, &plan)?);
    }
    let next_cursor = has_more
        .then(|| results.last())
        .flatten()
        .map(|result| encode_result_cursor(result, &plan))
        .transpose()?;

    Ok(FtsSearchPage {
        results,
        next_cursor,
    })
}

fn build_sql(plan: &query::SearchPlan, filter: &FtsSearchFilter) -> String {
    let rank_expression = match plan.sort {
        query::SortMode::Rank => "bm25(messages_fts)",
        query::SortMode::Time => "NULL",
    };
    let mut sql = String::from(
        "SELECT messages_fts.content AS indexed_content,
                m.msg_id, m.topic_id, m.owner_type, m.owner_id, m.role,
                m.agent_id AS speaker_agent_id, m.timestamp,
                t.title AS topic_title, ",
    );
    sql.push_str(rank_expression);
    sql.push_str(
        " AS match_rank
         FROM messages_fts
         INNER JOIN messages m ON messages_fts.owner_type = m.owner_type
            AND messages_fts.owner_id = m.owner_id
            AND messages_fts.topic_id = m.topic_id
            AND messages_fts.msg_id = m.msg_id
         INNER JOIN topics t ON m.owner_type = t.owner_type
            AND m.owner_id = t.owner_id
            AND m.topic_id = t.topic_id
         WHERE m.deleted_at IS NULL AND t.deleted_at IS NULL",
    );
    if plan.fts_match_query().is_some() {
        sql.push_str(" AND messages_fts.content MATCH ?");
    }
    for _ in &plan.short_terms {
        sql.push_str(" AND instr(lower(messages_fts.content), lower(?)) > 0");
    }
    append_filters(&mut sql, filter);
    append_cursor_filter(&mut sql, plan);
    match plan.sort {
        query::SortMode::Time => sql.push_str(
            " ORDER BY m.timestamp DESC, m.owner_type DESC, m.owner_id DESC,
                      m.topic_id DESC, m.msg_id DESC",
        ),
        query::SortMode::Rank => sql.push_str(
            " ORDER BY bm25(messages_fts) ASC, m.timestamp DESC,
                      m.owner_type DESC, m.owner_id DESC, m.topic_id DESC, m.msg_id DESC",
        ),
    }
    sql.push_str(" LIMIT ?");
    sql
}

fn append_filters(sql: &mut String, filter: &FtsSearchFilter) {
    if filter.topic_id.is_some() {
        sql.push_str(" AND m.topic_id = ?");
    }
    if filter.owner_type.is_some() {
        sql.push_str(" AND m.owner_type = ?");
    }
    if filter.owner_id.is_some() {
        sql.push_str(" AND m.owner_id = ?");
    }
    if filter.speaker_agent_id.is_some() {
        sql.push_str(" AND m.agent_id = ?");
    }
    if filter.role.is_some() {
        sql.push_str(" AND m.role = ?");
    }
    if filter.start_time.is_some() {
        sql.push_str(" AND m.timestamp >= ?");
    }
    if filter.end_time.is_some() {
        sql.push_str(" AND m.timestamp <= ?");
    }
}

fn append_cursor_filter(sql: &mut String, plan: &query::SearchPlan) {
    let Some(_cursor) = plan.cursor.as_ref() else {
        return;
    };
    match plan.sort {
        query::SortMode::Time => sql.push_str(
            " AND (m.timestamp, m.owner_type, m.owner_id, m.topic_id, m.msg_id)
                 < (?, ?, ?, ?, ?)",
        ),
        query::SortMode::Rank => sql.push_str(
            " AND (bm25(messages_fts) > ? OR
                 (bm25(messages_fts) = ? AND
                  (m.timestamp, m.owner_type, m.owner_id, m.topic_id, m.msg_id)
                  < (?, ?, ?, ?, ?)))",
        ),
    }
}

fn bind_query<'q>(
    mut query: sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    plan: &'q query::SearchPlan,
    filter: &'q FtsSearchFilter,
) -> sqlx::query::Query<'q, Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    if let Some(fts_query) = plan.fts_match_query() {
        query = query.bind(fts_query);
    }
    for term in &plan.short_terms {
        query = query.bind(term);
    }
    if let Some(topic_id) = filter.topic_id.as_ref() {
        query = query.bind(topic_id);
    }
    if let Some(owner_type) = filter.owner_type.as_ref() {
        query = query.bind(owner_type);
    }
    if let Some(owner_id) = filter.owner_id.as_ref() {
        query = query.bind(owner_id);
    }
    if let Some(speaker_agent_id) = filter.speaker_agent_id.as_ref() {
        query = query.bind(speaker_agent_id);
    }
    if let Some(role) = filter.role.as_ref() {
        query = query.bind(role);
    }
    if let Some(start_time) = filter.start_time {
        query = query.bind(start_time);
    }
    if let Some(end_time) = filter.end_time {
        query = query.bind(end_time);
    }
    if let Some(cursor) = plan.cursor.as_ref() {
        match plan.sort {
            query::SortMode::Time => {
                query = query
                    .bind(cursor.timestamp)
                    .bind(&cursor.owner_type)
                    .bind(&cursor.owner_id)
                    .bind(&cursor.topic_id)
                    .bind(&cursor.msg_id);
            }
            query::SortMode::Rank => {
                let rank = f64::from_bits(cursor.rank_bits.expect("相关度游标已完成校验"));
                query = query
                    .bind(rank)
                    .bind(rank)
                    .bind(cursor.timestamp)
                    .bind(&cursor.owner_type)
                    .bind(&cursor.owner_id)
                    .bind(&cursor.topic_id)
                    .bind(&cursor.msg_id);
            }
        }
    }
    query.bind(plan.limit + 1)
}

fn decode_result(
    row: sqlx::sqlite::SqliteRow,
    plan: &query::SearchPlan,
) -> Result<FtsSearchResult, String> {
    let indexed_content: String = row
        .try_get("indexed_content")
        .map_err(|error| format!("全文检索摘要读取失败: {error}"))?;
    let rank: Option<f64> = row
        .try_get("match_rank")
        .map_err(|error| format!("全文检索排序读取失败: {error}"))?;
    let topic_title: String = row
        .try_get("topic_title")
        .map_err(|error| format!("全文检索话题读取失败: {error}"))?;
    let terms = plan
        .long_terms
        .iter()
        .chain(plan.short_terms.iter())
        .cloned()
        .collect::<Vec<_>>();
    Ok(FtsSearchResult {
        msg_id: row
            .try_get("msg_id")
            .map_err(|error| format!("全文检索消息标识读取失败: {error}"))?,
        topic_id: row
            .try_get("topic_id")
            .map_err(|error| format!("全文检索话题标识读取失败: {error}"))?,
        owner_type: row
            .try_get("owner_type")
            .map_err(|error| format!("全文检索归属类型读取失败: {error}"))?,
        owner_id: row
            .try_get("owner_id")
            .map_err(|error| format!("全文检索归属标识读取失败: {error}"))?,
        role: row
            .try_get("role")
            .map_err(|error| format!("全文检索角色读取失败: {error}"))?,
        speaker_agent_id: row
            .try_get("speaker_agent_id")
            .map_err(|error| format!("全文检索发言 Agent 读取失败: {error}"))?,
        timestamp: row
            .try_get("timestamp")
            .map_err(|error| format!("全文检索时间读取失败: {error}"))?,
        topic_title: summary::summarize(&topic_title, &[]),
        snippet: summary::summarize(&indexed_content, &terms),
        rank: (plan.sort == query::SortMode::Rank).then(|| rank.unwrap_or_default()),
    })
}

fn encode_result_cursor(
    result: &FtsSearchResult,
    plan: &query::SearchPlan,
) -> Result<String, String> {
    cursor::encode_cursor(cursor::CursorPayload {
        version: 1,
        fingerprint: plan.fingerprint.clone(),
        sort: cursor::sort_name(plan.sort).to_string(),
        timestamp: result.timestamp,
        rank_bits: result.rank.map(f64::to_bits),
        owner_type: result.owner_type.clone(),
        owner_id: result.owner_id.clone(),
        topic_id: result.topic_id.clone(),
        msg_id: result.msg_id.clone(),
        checksum: String::new(),
    })
}
