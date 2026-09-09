use super::message_service_history::load_chat_history_internal;
use super::message_service_support::topic_key;
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::topic_types::TopicKey;
use tauri::{AppHandle, Manager};

const DEFAULT_CONTEXT_COUNT: usize = 10;
const MAX_CONTEXT_COUNT: usize = 100;
const ANCHOR_LOAD_ATTEMPTS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryWindow {
    limit: usize,
    offset: usize,
    has_more_history: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAroundResult {
    pub messages: Vec<ChatMessage>,
    pub next_offset: usize,
    pub has_more_history: bool,
}

async fn locate_history_window(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    anchor_message_id: &str,
    before_count: usize,
    after_count: usize,
) -> Result<Option<HistoryWindow>, String> {
    let anchor: Option<(i64, i64)> = sqlx::query_as(
        "SELECT m.timestamp, m.rowid
         FROM messages m
         INNER JOIN topics t ON t.owner_type = m.owner_type
            AND t.owner_id = m.owner_id AND t.topic_id = m.topic_id
         WHERE m.owner_type = ? AND m.owner_id = ? AND m.topic_id = ?
           AND m.msg_id = ? AND m.deleted_at IS NULL AND t.deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(anchor_message_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("定位搜索结果失败: {error}"))?;
    let Some((timestamp, row_id)) = anchor else {
        return Ok(None);
    };
    build_history_window(pool, key, timestamp, row_id, before_count, after_count)
        .await
        .map(Some)
}

async fn build_history_window(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    timestamp: i64,
    row_id: i64,
    before_count: usize,
    after_count: usize,
) -> Result<HistoryWindow, String> {
    let newer: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL
           AND (timestamp > ? OR (timestamp = ? AND rowid > ?))",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(timestamp)
    .bind(timestamp)
    .bind(row_id)
    .fetch_one(pool)
    .await
    .map_err(|error| format!("计算搜索结果上下文失败: {error}"))?;
    let newer = usize::try_from(newer).map_err(|_| "消息数量超出支持范围".to_string())?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .map_err(|error| format!("计算搜索结果历史范围失败: {error}"))?;
    let total = usize::try_from(total).map_err(|_| "消息数量超出支持范围".to_string())?;
    let before_count = before_count.min(MAX_CONTEXT_COUNT);
    let after_count = after_count.min(MAX_CONTEXT_COUNT);
    let offset = newer.saturating_sub(after_count);
    let included_newer = newer - offset;
    let limit = included_newer + 1 + before_count;
    Ok(HistoryWindow {
        limit,
        offset,
        has_more_history: total > offset.saturating_add(limit),
    })
}

/** 使用完整会话身份加载搜索锚点附近的有界历史。 */
pub async fn load_chat_history_around_internal(
    app_handle: &AppHandle,
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    anchor_message_id: &str,
    before_count: Option<usize>,
    after_count: Option<usize>,
) -> Result<HistoryAroundResult, String> {
    if anchor_message_id.is_empty() {
        return Err("anchorMessageId 不能为空".to_string());
    }
    let key = topic_key(owner_id, owner_type, topic_id)?;
    let pool = &app_handle
        .state::<crate::vcp_modules::db_manager::DbState>()
        .pool;
    for _ in 0..ANCHOR_LOAD_ATTEMPTS {
        let Some(window) = locate_history_window(
            pool,
            &key,
            anchor_message_id,
            before_count.unwrap_or(DEFAULT_CONTEXT_COUNT),
            after_count.unwrap_or(DEFAULT_CONTEXT_COUNT),
        )
        .await?
        else {
            return Ok(HistoryAroundResult {
                messages: Vec::new(),
                next_offset: 0,
                has_more_history: false,
            });
        };
        let history = load_chat_history_internal(
            app_handle,
            owner_id,
            owner_type,
            topic_id,
            Some(window.limit),
            Some(window.offset),
            false,
            false,
        )
        .await?;
        let verified_window = locate_history_window(
            pool,
            &key,
            anchor_message_id,
            before_count.unwrap_or(DEFAULT_CONTEXT_COUNT),
            after_count.unwrap_or(DEFAULT_CONTEXT_COUNT),
        )
        .await?;
        if verified_window == Some(window)
            && history
                .iter()
                .any(|message| message.id == anchor_message_id)
        {
            return Ok(HistoryAroundResult {
                next_offset: window.offset.saturating_add(history.len()),
                has_more_history: window.has_more_history,
                messages: history,
            });
        }
    }
    Err("目标消息在加载期间发生变化，请重试".to_string())
}

#[cfg(test)]
mod tests {
    use super::{locate_history_window, HistoryWindow};
    use crate::vcp_modules::topic_types::TopicKey;

    async fn fixture() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("应打开锚点测试数据库");
        sqlx::raw_sql(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, deleted_at INTEGER
             );
             CREATE TABLE messages (
                owner_type TEXT, owner_id TEXT, topic_id TEXT, msg_id TEXT,
                timestamp INTEGER, deleted_at INTEGER
             );
             INSERT INTO topics VALUES ('agent', 'shared', 'topic', NULL);
             INSERT INTO topics VALUES ('group', 'shared', 'topic', NULL);
             INSERT INTO messages VALUES ('agent', 'shared', 'topic', 'older', 99, NULL);
             INSERT INTO messages VALUES ('agent', 'shared', 'topic', 'tie-older', 100, NULL);
             INSERT INTO messages VALUES ('agent', 'shared', 'topic', 'anchor', 100, NULL);
             INSERT INTO messages VALUES ('agent', 'shared', 'topic', 'tie-newer', 100, NULL);
             INSERT INTO messages VALUES ('agent', 'shared', 'topic', 'newer', 101, NULL);
             INSERT INTO messages VALUES ('group', 'shared', 'topic', 'anchor', 999, NULL);",
        )
        .execute(&pool)
        .await
        .expect("应创建锚点测试数据");
        pool
    }

    #[tokio::test]
    async fn 窗口使用完整归属身份和行号打破时间并列() {
        let pool = fixture().await;
        let key = TopicKey::new("agent", "shared", "topic");
        let window = locate_history_window(&pool, &key, "anchor", 1, 1)
            .await
            .expect("应定位锚点");
        assert_eq!(
            window,
            Some(HistoryWindow {
                limit: 3,
                offset: 1,
                has_more_history: true,
            })
        );
    }

    #[tokio::test]
    async fn 已删除锚点和话题不可加载() {
        let pool = fixture().await;
        let key = TopicKey::new("agent", "shared", "topic");
        sqlx::query(
            "UPDATE messages SET deleted_at = 1 WHERE msg_id = 'anchor' AND owner_type = 'agent'",
        )
        .execute(&pool)
        .await
        .expect("应删除锚点");
        assert_eq!(
            locate_history_window(&pool, &key, "anchor", 10, 10)
                .await
                .expect("应检查已删除锚点"),
            None
        );
        sqlx::query("UPDATE topics SET deleted_at = 1 WHERE owner_type = 'group'")
            .execute(&pool)
            .await
            .expect("应删除群组话题");
        let group = TopicKey::new("group", "shared", "topic");
        assert_eq!(
            locate_history_window(&pool, &group, "anchor", 10, 10)
                .await
                .expect("应检查已删除话题"),
            None
        );
    }

    #[tokio::test]
    async fn 上下文数量受到上限约束() {
        let pool = fixture().await;
        let key = TopicKey::new("agent", "shared", "topic");
        let window = locate_history_window(&pool, &key, "anchor", usize::MAX, usize::MAX)
            .await
            .expect("应定位受限锚点")
            .expect("锚点应存在");
        assert_eq!(
            window,
            HistoryWindow {
                limit: 103,
                offset: 0,
                has_more_history: false,
            }
        );
    }
}
