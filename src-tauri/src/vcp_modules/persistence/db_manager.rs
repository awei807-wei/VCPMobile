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
    pub owner_type: String,
    pub owner_id: String,
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
    /// 会话归属过滤；与 topic_id 一起使用时必须成对提供。
    pub owner_id: Option<String>,
    pub owner_type: Option<String>,
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
    if filter.owner_id.is_some() != filter.owner_type.is_some() {
        return Err("search owner filter requires both ownerId and ownerType".to_string());
    }
    if filter.topic_id.is_some() && filter.owner_id.is_none() {
        return Err("topic search requires both ownerId and ownerType".to_string());
    }
    if filter
        .owner_type
        .as_deref()
        .is_some_and(|owner_type| !matches!(owner_type, "agent" | "group"))
    {
        return Err("search ownerType must be agent or group".to_string());
    }
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
            m.owner_type,
            m.owner_id,
            m.role, 
            m.content, 
            m.timestamp, 
            t.title AS topic_title
         FROM messages_fts fts
         INNER JOIN messages m ON fts.owner_type = m.owner_type
            AND fts.owner_id = m.owner_id
            AND fts.topic_id = m.topic_id
            AND fts.msg_id = m.msg_id
         INNER JOIN topics t ON m.owner_type = t.owner_type
            AND m.owner_id = t.owner_id
            AND m.topic_id = t.topic_id
         WHERE fts.content MATCH ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL",
    );
    if filter.topic_id.is_some() {
        sql.push_str(" AND m.topic_id = ?");
    }
    if filter.owner_id.is_some() {
        sql.push_str(" AND m.owner_id = ? AND m.owner_type = ?");
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
    if let Some(ref owner_id) = filter.owner_id {
        query = query.bind(owner_id);
    }
    if let Some(ref owner_type) = filter.owner_type {
        query = query.bind(owner_type);
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
        let owner_type: String = row.get("owner_type");
        let owner_id: String = row.get("owner_id");
        let role: String = row.get("role");
        let content = super::message_content_storage::decode_message_content(&row, "content")?;
        let timestamp: i64 = row.get("timestamp");
        let topic_title: String = row.get("topic_title");

        results.push(FtsSearchResult {
            msg_id,
            topic_id,
            owner_type,
            owner_id,
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

    #[test]
    fn composite_fts_join_contains_the_complete_message_identity() {
        let filter = FtsSearchFilter {
            query: "needle".to_string(),
            topic_id: Some("shared-topic".to_string()),
            owner_id: Some("owner-a".to_string()),
            owner_type: Some("agent".to_string()),
            agent_id: None,
            role: None,
            start_time: None,
            end_time: None,
            limit: Some(10),
        };
        let sql = build_fts_sql(&filter);
        assert!(sql.contains("fts.owner_type = m.owner_type"));
        assert!(sql.contains("fts.owner_id = m.owner_id"));
        assert!(sql.contains("m.owner_type = t.owner_type"));
        assert!(sql.contains("m.owner_id = t.owner_id"));
        assert!(sql.contains("m.owner_id = ? AND m.owner_type = ?"));
    }

    #[tokio::test]
    async fn fts_search_owner_filter_does_not_cross_same_topic_or_message_id() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open FTS test database");
        sqlx::raw_sql(
            "CREATE TABLE topics (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                title TEXT NOT NULL,
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
             );
             CREATE TABLE messages (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                msg_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content BLOB NOT NULL,
                timestamp INTEGER NOT NULL,
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
             );
             CREATE VIRTUAL TABLE messages_fts USING fts5(
                msg_id UNINDEXED,
                topic_id UNINDEXED,
                content,
                owner_type UNINDEXED,
                owner_id UNINDEXED
             );",
        )
        .execute(&pool)
        .await
        .expect("create FTS schema");

        for (owner_type, owner_id, title, content) in [
            ("agent", "owner-a", "Agent topic", "agent needle"),
            ("group", "owner-g", "Group topic", "group needle"),
        ] {
            sqlx::query(
                "INSERT INTO topics(owner_type, owner_id, topic_id, title)
                 VALUES (?, ?, 'shared-topic', ?)",
            )
            .bind(owner_type)
            .bind(owner_id)
            .bind(title)
            .execute(&pool)
            .await
            .expect("insert FTS topic");
            let compressed = super::super::message_repository::ContentCompressor::compress(content)
                .expect("compress FTS message");
            sqlx::query(
                "INSERT INTO messages(
                    owner_type, owner_id, topic_id, msg_id, role, content, timestamp
                 ) VALUES (?, ?, 'shared-topic', 'shared-message', 'user', ?, 100)",
            )
            .bind(owner_type)
            .bind(owner_id)
            .bind(compressed)
            .execute(&pool)
            .await
            .expect("insert FTS message");
            sqlx::query(
                "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
                 VALUES ('shared-message', 'shared-topic', ?, ?, ?)",
            )
            .bind(content)
            .bind(owner_type)
            .bind(owner_id)
            .execute(&pool)
            .await
            .expect("insert FTS index row");
        }

        let filter = FtsSearchFilter {
            query: "needle".to_string(),
            topic_id: Some("shared-topic".to_string()),
            owner_id: Some("owner-a".to_string()),
            owner_type: Some("agent".to_string()),
            agent_id: None,
            role: None,
            start_time: None,
            end_time: None,
            limit: Some(10),
        };
        let fts_query = compile_fts_query(&filter.query).expect("compile FTS query");
        let rows = fetch_fts_rows(&pool, &build_fts_sql(&filter), &fts_query, &filter)
            .await
            .expect("execute owner-scoped FTS query");
        let results = decode_fts_rows(rows).expect("decode owner-scoped FTS results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].owner_type, "agent");
        assert_eq!(results[0].owner_id, "owner-a");
        assert_eq!(results[0].topic_id, "shared-topic");
        assert_eq!(results[0].msg_id, "shared-message");
        assert_eq!(results[0].content, "agent needle");
    }
}
