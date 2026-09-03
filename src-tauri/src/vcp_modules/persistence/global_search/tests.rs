use super::{query::SearchPlan, search_messages, FtsSearchFilter};
use sqlx::SqlitePool;

fn request(query: &str) -> FtsSearchFilter {
    FtsSearchFilter {
        query: query.to_string(),
        topic_id: None,
        owner_id: None,
        owner_type: None,
        speaker_agent_id: None,
        role: None,
        start_time: None,
        end_time: None,
        limit: None,
        sort: None,
        cursor: None,
    }
}

async fn test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("应打开全局搜索测试数据库");
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
            agent_id TEXT,
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
            owner_id UNINDEXED,
            tokenize='trigram'
         );",
    )
    .execute(&pool)
    .await
    .expect("应创建全局搜索测试结构");
    pool
}

async fn insert_message(
    pool: &SqlitePool,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    msg_id: &str,
    speaker_agent_id: Option<&str>,
    content: &str,
    timestamp: i64,
    deleted_at: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO topics(owner_type, owner_id, topic_id, title)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(owner_type, owner_id, topic_id) DO NOTHING",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(topic_id)
    .bind(format!("{owner_type} topic"))
    .execute(pool)
    .await
    .expect("应插入搜索话题");
    sqlx::query(
        "INSERT INTO messages(
            owner_type, owner_id, topic_id, msg_id, role, agent_id, content, timestamp, deleted_at
         ) VALUES (?, ?, ?, ?, 'user', ?, ?, ?, ?)",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(topic_id)
    .bind(msg_id)
    .bind(speaker_agent_id)
    .bind(content.as_bytes())
    .bind(timestamp)
    .bind(deleted_at)
    .execute(pool)
    .await
    .expect("应插入搜索消息");
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(msg_id)
    .bind(topic_id)
    .bind(content)
    .bind(owner_type)
    .bind(owner_id)
    .execute(pool)
    .await
    .expect("应插入搜索索引行");
}

async fn 准备搜索测试数据() -> SqlitePool {
    let pool = test_pool().await;
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "shared-topic",
        "message-a1",
        Some("speaker-a"),
        "needle agent short 二字",
        100,
        None,
    )
    .await;
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "shared-topic",
        "message-a2",
        Some("speaker-a"),
        "needle agent second",
        100,
        None,
    )
    .await;
    insert_message(
        &pool,
        "group",
        "owner-g",
        "shared-topic",
        "message-g1",
        Some("speaker-g"),
        "needle group short 二字",
        100,
        None,
    )
    .await;
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "shared-topic",
        "message-deleted",
        Some("speaker-a"),
        "needle deleted",
        100,
        Some(200),
    )
    .await;
    pool
}

#[test]
fn 短词和混合查询只使用未压缩的索引正文列() {
    let short = SearchPlan::from_filter(&request("二字")).expect("短词查询应有效");
    let sql = super::build_sql(&short, &request("二字"));
    assert!(sql.contains("instr(messages_fts.content, ?) > 0"));
    assert!(!sql.contains("instr(m.content"));

    let mixed_request = request("二字 needle");
    let mixed = SearchPlan::from_filter(&mixed_request).expect("混合查询应有效");
    let sql = super::build_sql(&mixed, &mixed_request);
    assert!(sql.contains("messages_fts.content MATCH ?"));
    assert!(sql.contains("instr(messages_fts.content, ?) > 0"));
    assert!(!sql.contains("messages.content"));

    let mut short_rank_request = request("二字");
    short_rank_request.sort = Some("rank".to_string());
    let short_rank = SearchPlan::from_filter(&short_rank_request).expect("短词相关度查询应有效");
    let sql = super::build_sql(&short_rank, &short_rank_request);
    assert!(!sql.contains("bm25(messages_fts)"));
}

#[tokio::test]
async fn 归属筛选发言者筛选和删除过滤彼此独立() {
    let pool = 准备搜索测试数据().await;

    let all = search_messages(&pool, request("needle"))
        .await
        .expect("应搜索全部归属空间");
    assert_eq!(all.results.len(), 3);
    assert!(all
        .results
        .iter()
        .all(|result| result.msg_id != "message-deleted"));

    let mut owner_type_only = request("needle");
    owner_type_only.owner_type = Some("agent".to_string());
    assert_eq!(
        search_messages(&pool, owner_type_only)
            .await
            .unwrap()
            .results
            .len(),
        2
    );

    let mut owner_tuple = request("needle");
    owner_tuple.owner_type = Some("agent".to_string());
    owner_tuple.owner_id = Some("owner-a".to_string());
    owner_tuple.topic_id = Some("shared-topic".to_string());
    let scoped = search_messages(&pool, owner_tuple).await.unwrap();
    assert_eq!(scoped.results.len(), 2);
    assert!(scoped
        .results
        .iter()
        .all(|result| result.owner_id == "owner-a"));

    let mut speaker = request("needle");
    speaker.speaker_agent_id = Some("speaker-g".to_string());
    let speaker_results = search_messages(&pool, speaker).await.unwrap();
    assert_eq!(speaker_results.results.len(), 1);
    assert_eq!(speaker_results.results[0].owner_type, "group");
    assert_eq!(
        speaker_results.results[0].speaker_agent_id.as_deref(),
        Some("speaker-g")
    );
}

#[tokio::test]
async fn 短词和混合统一码查询匹配索引正文() {
    let pool = 准备搜索测试数据().await;

    let short = search_messages(&pool, request("二字")).await.unwrap();
    assert_eq!(short.results.len(), 2);
    assert!(short
        .results
        .iter()
        .all(|result| result.snippet.contains("二字")));

    let mixed = search_messages(&pool, request("二字 needle"))
        .await
        .unwrap();
    assert_eq!(mixed.results.len(), 2);

    let emoji = request("😀😀");
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "emoji-topic",
        "emoji-message",
        Some("speaker-a"),
        "prefix 😀😀 suffix",
        90,
        None,
    )
    .await;
    let emoji_results = search_messages(&pool, emoji).await.unwrap();
    assert_eq!(emoji_results.results.len(), 1);
}

#[tokio::test]
async fn 单双三字英文和特殊字符均按字面量查询() {
    let pool = test_pool().await;
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "literal-topic",
        "literal-message",
        None,
        "一 二字 三个字 English a.b [方括号] 双\"引号",
        1,
        None,
    )
    .await;

    for query in ["一", "二字", "三个字", "English", "a.b", "双\"引号"] {
        let page = search_messages(&pool, request(query))
            .await
            .unwrap_or_else(|error| panic!("字面量查询 {query} 应成功: {error}"));
        assert_eq!(page.results.len(), 1, "字面量查询 {query} 应命中一次");
    }
}

#[tokio::test]
async fn 日期角色和已删除话题筛选共同生效() {
    let pool = 准备搜索测试数据().await;
    sqlx::query(
        "UPDATE messages SET role = 'assistant', timestamp = 150
         WHERE owner_type = 'agent' AND msg_id = 'message-a1'",
    )
    .execute(&pool)
    .await
    .expect("应更新筛选测试消息");
    sqlx::query(
        "UPDATE topics SET deleted_at = 200
         WHERE owner_type = 'group' AND owner_id = 'owner-g'",
    )
    .execute(&pool)
    .await
    .expect("应逻辑删除筛选测试话题");

    let mut filter = request("needle");
    filter.role = Some("assistant".to_string());
    filter.start_time = Some(150);
    filter.end_time = Some(150);
    let page = search_messages(&pool, filter)
        .await
        .expect("组合筛选应成功");
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].msg_id, "message-a1");

    let all = search_messages(&pool, request("needle"))
        .await
        .expect("删除话题后搜索应成功");
    assert!(all
        .results
        .iter()
        .all(|result| result.owner_type == "agent"));
}

#[tokio::test]
async fn 时间游标使用完整归属身份且无重复遗漏() {
    let pool = 准备搜索测试数据().await;
    let mut first_request = request("needle");
    first_request.limit = Some(2);
    let first = search_messages(&pool, first_request.clone()).await.unwrap();
    assert_eq!(first.results.len(), 2);
    let cursor = first.next_cursor.clone().expect("第一页应返回下一页游标");

    let mut second_request = first_request;
    second_request.cursor = Some(cursor);
    let second = search_messages(&pool, second_request).await.unwrap();
    assert_eq!(second.results.len(), 1);
    assert!(second.next_cursor.is_none());

    let ids = first
        .results
        .iter()
        .chain(second.results.iter())
        .map(|result| result.msg_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
}

#[tokio::test]
async fn 相关度游标和游标完整性得到校验() {
    let pool = 准备搜索测试数据().await;
    let mut first_request = request("needle");
    first_request.limit = Some(1);
    first_request.sort = Some("rank".to_string());
    let first = search_messages(&pool, first_request.clone()).await.unwrap();
    let cursor = first
        .next_cursor
        .clone()
        .expect("相关度第一页应返回下一页游标");

    let mut second_request = first_request.clone();
    second_request.cursor = Some(cursor.clone());
    let second = search_messages(&pool, second_request).await.unwrap();
    assert_eq!(second.results.len(), 1);
    assert_ne!(first.results[0].msg_id, second.results[0].msg_id);

    let mut tampered = first_request.clone();
    tampered.cursor = Some(format!("{cursor}A"));
    assert!(search_messages(&pool, tampered).await.is_err());

    let mut other_query = first_request;
    other_query.query = "other".to_string();
    other_query.cursor = Some(cursor);
    assert!(search_messages(&pool, other_query).await.is_err());
}

#[tokio::test]
async fn 短词相关度排序稳定分页且无重复遗漏() {
    let pool = 准备搜索测试数据().await;
    let mut page_request = request("二字");
    page_request.limit = Some(1);
    page_request.sort = Some("rank".to_string());
    let mut ids = Vec::new();
    loop {
        let page = search_messages(&pool, page_request.clone())
            .await
            .expect("短词分页查询应成功");
        ids.extend(page.results.iter().map(|result| result.msg_id.clone()));
        let Some(cursor) = page.next_cursor else {
            break;
        };
        page_request.cursor = Some(cursor);
    }
    assert_eq!(ids.len(), 2);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        2
    );

    let mut different_sort = request("二字");
    different_sort.cursor = page_request.cursor;
    assert!(search_messages(&pool, different_sort).await.is_err());
}

#[tokio::test]
async fn 结果只包含有界纯文本摘要() {
    let pool = test_pool().await;
    let content = format!("prefix <mark>needle</mark> {}", "正文".repeat(300));
    insert_message(
        &pool,
        "agent",
        "owner-a",
        "topic",
        "message",
        Some("speaker-a"),
        &content,
        1,
        None,
    )
    .await;
    let page = search_messages(&pool, request("needle")).await.unwrap();
    let result = &page.results[0];
    assert!(result.snippet.chars().count() <= 240);
    assert!(!result.snippet.contains('<'));
    assert!(!result.snippet.contains('>'));
    assert!(serde_json::to_value(result)
        .unwrap()
        .get("content")
        .is_none());
}
