use sqlx::{sqlite::SqliteRow, Pool, Row, Sqlite};
use tauri::{AppHandle, Manager, Runtime, State};

pub const CORE_NOT_READY_ERROR: &str = "CORE_NOT_READY: 数据库尚未初始化，请稍后重试。";

pub struct DbState {
    pub pool: Pool<Sqlite>,
    pub path: std::path::PathBuf,
}

/// 获取已完成初始化的数据库状态；启动竞态期间返回可恢复错误而不是触发 panic。
pub fn require_db_state<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<State<'_, DbState>, String> {
    app_handle
        .try_state::<DbState>()
        .ok_or_else(|| CORE_NOT_READY_ERROR.to_string())
}

impl DbState {
    /// 执行 SQLite 物理页面碎片分批回收与查询规划器索引优化
    pub async fn run_incremental_vacuum_optimize(
        &self,
        pages_to_vacuum: i32,
    ) -> Result<(), sqlx::Error> {
        // 1. 分批页整理碎片，防堵大面积 I/O 阻塞
        sqlx::query(&format!("PRAGMA incremental_vacuum({})", pages_to_vacuum))
            .execute(&self.pool)
            .await?;
        // 2. 重构索引规划器
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        Ok(())
    }
}

pub async fn init_db(app_handle: &AppHandle) -> Result<(Pool<Sqlite>, std::path::PathBuf), String> {
    super::database_lifecycle::init_db(app_handle).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchResult {
    pub msg_id: String,
    pub topic_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub topic_title: String,
}

pub fn preprocess_fts_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    let mut last_was_cjk = false;

    for c in text.chars() {
        let is_cjk = ('\u{4e00}'..='\u{9fff}').contains(&c)
            || ('\u{3400}'..='\u{4dbf}').contains(&c)
            || ('\u{20000}'..='\u{2a6df}').contains(&c);

        if is_cjk {
            if !result.is_empty() && !last_was_cjk && !result.ends_with(' ') {
                result.push(' ');
            }
            result.push(c);
            result.push(' ');
            last_was_cjk = true;
        } else {
            if last_was_cjk && c != ' ' && !result.ends_with(' ') {
                result.push(' ');
            }
            result.push(c);
            last_was_cjk = false;
        }
    }
    result.trim().to_string()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsSearchFilter {
    pub query: String,
    pub topic_id: Option<String>,
    pub agent_id: Option<String>,
    pub role: Option<String>,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub limit: Option<i64>,
}

#[tauri::command]
pub async fn search_messages_fts(
    db_state: tauri::State<'_, DbState>,
    filter: FtsSearchFilter,
) -> Result<Vec<FtsSearchResult>, String> {
    let Some(fts_query) = compile_fts_query(&filter.query) else {
        return Ok(Vec::new());
    };
    let sql = build_fts_sql(&filter);
    let rows = fetch_fts_rows(&db_state.pool, &sql, &fts_query, &filter).await?;
    decode_fts_rows(rows)
}

fn compile_fts_query(query: &str) -> Option<String> {
    let processed = preprocess_fts_text(query.trim());
    let terms: Vec<String> = processed
        .split_whitespace()
        .map(|s| format!("\"{}\"", s))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.join(" AND "))
}

fn build_fts_sql(filter: &FtsSearchFilter) -> String {
    let mut sql = String::from(
        "SELECT 
            m.msg_id, 
            m.topic_id, 
            m.role, 
            m.content, 
            m.timestamp, 
            t.title AS topic_title
         FROM messages_fts fts
         INNER JOIN messages m ON fts.msg_id = m.msg_id AND fts.topic_id = m.topic_id
         INNER JOIN topics t ON m.topic_id = t.topic_id
         WHERE fts.content MATCH ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL",
    );
    if filter.topic_id.is_some() {
        sql.push_str(" AND m.topic_id = ?");
    }
    if filter.agent_id.is_some() {
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
    sql.push_str(" ORDER BY m.timestamp DESC LIMIT ?");
    sql
}

async fn fetch_fts_rows(
    pool: &Pool<Sqlite>,
    sql: &str,
    fts_query: &str,
    filter: &FtsSearchFilter,
) -> Result<Vec<SqliteRow>, String> {
    let mut query = sqlx::query(sql).bind(fts_query);
    if let Some(ref topic_id) = filter.topic_id {
        query = query.bind(topic_id);
    }
    if let Some(ref agent_id) = filter.agent_id {
        query = query.bind(agent_id);
    }
    if let Some(ref role) = filter.role {
        query = query.bind(role);
    }
    if let Some(start_time) = filter.start_time {
        query = query.bind(start_time);
    }
    if let Some(end_time) = filter.end_time {
        query = query.bind(end_time);
    }
    query
        .bind(filter.limit.unwrap_or(100))
        .fetch_all(pool)
        .await
        .map_err(|error| format!("全文检索执行失败: {error}"))
}

fn decode_fts_rows(rows: Vec<SqliteRow>) -> Result<Vec<FtsSearchResult>, String> {
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        let msg_id: String = row.get("msg_id");
        let topic_id: String = row.get("topic_id");
        let role: String = row.get("role");
        let content = super::message_content_storage::decode_message_content(&row, "content")?;
        let timestamp: i64 = row.get("timestamp");
        let topic_title: String = row.get("topic_title");

        results.push(FtsSearchResult {
            msg_id,
            topic_id,
            role,
            content,
            timestamp,
            topic_title,
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_fts_text() {
        assert_eq!(preprocess_fts_text("我喜欢AI"), "我 喜 欢 AI");
        assert_eq!(preprocess_fts_text("AI智能体"), "AI 智 能 体");
        assert_eq!(preprocess_fts_text("Hello World"), "Hello World");
        assert_eq!(preprocess_fts_text(""), "");
    }
}
