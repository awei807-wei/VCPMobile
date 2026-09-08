use super::{finalize::clear_active_generation, update_existing_message_content_for_pool};
use crate::vcp_modules::chat_manager::ChatMessage;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};

#[tokio::test]
async fn 清理活动生成失败会向调用方传播() {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("创建内存数据库");
    let key = MessageKey::new(TopicKey::new("agent", "owner", "topic"), "message");

    let error = clear_active_generation(&pool, &key)
        .await
        .expect_err("缺少表时必须返回数据库错误");
    assert!(error.contains("清理 active_generations 失败"));
}

#[tokio::test]
async fn 直接正文更新同步正文哈希缓存与全文检索() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("创建内存数据库");
    sqlx::raw_sql(
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT NOT NULL DEFAULT '', config_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER,
            updated_at INTEGER NOT NULL DEFAULT 0,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, name TEXT, content_hash TEXT NOT NULL DEFAULT '', deleted_at INTEGER
         );
         CREATE TABLE active_generations (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content BLOB NOT NULL, timestamp INTEGER NOT NULL,
            is_group_message INTEGER NOT NULL DEFAULT 0, group_id TEXT,
            finish_reason TEXT, content_hash TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE render_cache (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, render_content BLOB, updated_at INTEGER NOT NULL,
            content_hash TEXT NOT NULL DEFAULT '', renderer_schema_version INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
            display_name TEXT NOT NULL, src TEXT, status TEXT, created_at INTEGER NOT NULL,
            deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id, attachment_order)
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
            internal_path TEXT NOT NULL, extracted_text TEXT, image_frames TEXT,
            thumbnail_path TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         CREATE TABLE messages_fts (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, content TEXT NOT NULL,
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL
         );",
    )
    .execute(&pool)
    .await
    .expect("创建消息表");
    sqlx::query("INSERT INTO agents(agent_id) VALUES ('owner')")
        .execute(&pool)
        .await
        .expect("写入代理");
    sqlx::query(
        "INSERT INTO topics(owner_type, owner_id, topic_id)
         VALUES ('agent', 'owner', 'topic')",
    )
    .execute(&pool)
    .await
    .expect("写入话题");
    sqlx::query(
        "INSERT INTO messages
         (owner_type, owner_id, topic_id, msg_id, role, content, timestamp)
         VALUES ('agent', 'owner', 'topic', 'message', 'assistant', ?, 1)",
    )
    .bind("旧正文")
    .execute(&pool)
    .await
    .expect("写入消息");
    sqlx::query(
        "INSERT INTO active_generations(owner_type, owner_id, topic_id, msg_id, created_at)
         VALUES ('agent', 'owner', 'topic', 'message', 1)",
    )
    .execute(&pool)
    .await
    .expect("写入活动生成");

    let topic_key = TopicKey::new("agent", "owner", "topic");
    let message = ChatMessage {
        id: "message".to_string(),
        role: "assistant".to_string(),
        content: "新的正文".to_string(),
        timestamp: 1,
        is_thinking: Some(false),
        agent_id: Some("owner".to_string()),
        topic_id: Some("topic".to_string()),
        is_group_message: Some(false),
        finish_reason: Some("completed".to_string()),
        ..Default::default()
    };
    update_existing_message_content_for_pool(
        &pool,
        &MessageKey::new(topic_key.clone(), "message"),
        message.content.clone(),
        message.finish_reason.clone(),
        false,
    )
    .await
    .expect("通过生产正文更新包装器");

    let hashes: (String, String) = sqlx::query_as(
        "SELECT m.content_hash, r.content_hash
         FROM messages m JOIN render_cache r
           ON r.owner_type = m.owner_type AND r.owner_id = m.owner_id
          AND r.topic_id = m.topic_id AND r.msg_id = m.msg_id
         WHERE m.owner_type = 'agent' AND m.owner_id = 'owner'
           AND m.topic_id = 'topic' AND m.msg_id = 'message'",
    )
    .fetch_one(&pool)
    .await
    .expect("读取正文哈希");
    assert!(!hashes.0.is_empty());
    assert_eq!(hashes.0, hashes.1);
    let cached_blocks: Vec<u8> = sqlx::query_scalar(
        "SELECT render_content FROM render_cache
         WHERE owner_type = 'agent' AND owner_id = 'owner'
           AND topic_id = 'topic' AND msg_id = 'message'",
    )
    .fetch_one(&pool)
    .await
    .expect("读取渲染缓存");
    assert!(!cached_blocks.is_empty());
    let indexed_content: String = sqlx::query_scalar(
        "SELECT content FROM messages_fts
         WHERE owner_type = 'agent' AND owner_id = 'owner'
           AND topic_id = 'topic' AND msg_id = 'message'",
    )
    .fetch_one(&pool)
    .await
    .expect("读取全文索引");
    assert_eq!(indexed_content, "新的正文");
}
