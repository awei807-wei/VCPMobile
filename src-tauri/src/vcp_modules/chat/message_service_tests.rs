use super::{
    delete_message_attachment_for_key, delete_messages_for_topic,
    edit_message_and_truncate_history, edit_message_and_truncate_history_with_loaded_attachments,
    load_multi_topic_messages, load_multi_topic_messages_for_keys,
    truncate_history_after_timestamp_for_topic,
};
use crate::vcp_modules::chat::topic_service::{
    record_topic_unread_for_message_in_pool, set_topic_unread_in_pool,
};
use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::infra::file_manager::get_attachments_root_dir;
use crate::vcp_modules::infra::utils::calculate_sha256;
use crate::vcp_modules::topic_types::TopicKey;
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

async fn test_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open test database");
    sqlx::raw_sql(
        "CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT 'before',
            deleted_at INTEGER
         );
         CREATE TABLE groups (
            group_id TEXT PRIMARY KEY, content_hash TEXT NOT NULL DEFAULT 'before',
            deleted_at INTEGER
         );
         CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0, unread INTEGER NOT NULL DEFAULT 0,
            unread_count INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            last_message_updated_at INTEGER NOT NULL DEFAULT 0,
            msg_count INTEGER NOT NULL DEFAULT 0,
            config_hash TEXT NOT NULL DEFAULT 'before',
            content_hash TEXT NOT NULL DEFAULT 'before', deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, role TEXT NOT NULL, name TEXT, agent_id TEXT,
            content BLOB NOT NULL, timestamp INTEGER NOT NULL,
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0, is_group_message INTEGER NOT NULL DEFAULT 0,
            group_id TEXT, finish_reason TEXT, content_hash TEXT NOT NULL DEFAULT '',
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE render_cache (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, render_content BLOB, updated_at INTEGER NOT NULL DEFAULT 0,
            content_hash TEXT NOT NULL DEFAULT '', renderer_schema_version INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL,
            display_name TEXT NOT NULL, src TEXT, status TEXT, created_at INTEGER NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id, msg_id, attachment_order)
         );
         CREATE TABLE active_generations (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, created_at INTEGER NOT NULL,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
        CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
            internal_path TEXT NOT NULL, extracted_text TEXT, image_frames TEXT,
            thumbnail_path TEXT, created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE messages_fts (
            msg_id TEXT NOT NULL, topic_id TEXT NOT NULL, content TEXT NOT NULL,
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL
         );
         CREATE TABLE message_unread_receipts (
            owner_type TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL,
            created_at BIGINT NOT NULL,
            counted_unread INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );",
    )
    .execute(&pool)
    .await
    .expect("create message service schema");
    pool
}

async fn insert_message(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    content: &str,
    with_attachment: bool,
) {
    let compressed = zstd::bulk::compress(content.as_bytes(), 3).expect("compress message");
    let (table, column) = match key.owner_type.as_str() {
        "agent" => ("agents", "agent_id"),
        "group" => ("groups", "group_id"),
        other => panic!("unsupported test owner type {other}"),
    };
    sqlx::query(&format!(
        "INSERT OR IGNORE INTO {table}({column}) VALUES (?)"
    ))
    .bind(&key.owner_id)
    .execute(pool)
    .await
    .expect("insert owner");
    sqlx::query("INSERT INTO topics(owner_type, owner_id, topic_id) VALUES (?, ?, ?)")
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(pool)
        .await
        .expect("insert topic");
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, role, content, timestamp, updated_at)
         VALUES (?, ?, ?, 'same-message', 'user', ?, 100, 101)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(compressed)
    .execute(pool)
    .await
    .expect("insert message");
    sqlx::query(
        "INSERT INTO render_cache(owner_type, owner_id, topic_id, msg_id, render_content)
         VALUES (?, ?, ?, 'same-message', ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(Vec::<u8>::new())
    .execute(pool)
    .await
    .expect("insert cache");
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES ('same-message', ?, ?, ?, ?)",
    )
    .bind(&key.topic_id)
    .bind(content)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(pool)
    .await
    .expect("insert fts");
    if with_attachment {
        sqlx::query(
            "INSERT INTO attachments(hash, mime_type, size, internal_path, created_at)
             VALUES ('hash-a', 'text/plain', 1, '', 100)",
        )
        .execute(pool)
        .await
        .expect("insert attachment");
        sqlx::query(
            "INSERT INTO message_attachments(
                owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
                display_name, created_at
             ) VALUES (?, ?, ?, 'same-message', 'hash-a', 7, 'ordered.txt', 100)",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(pool)
        .await
        .expect("insert message attachment");
    }
}

async fn insert_extra_message(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    message_id: &str,
    timestamp: i64,
    active: bool,
) {
    let compressed = zstd::bulk::compress(message_id.as_bytes(), 3).expect("压缩测试消息");
    sqlx::query(
        "INSERT INTO messages(
            owner_type, owner_id, topic_id, msg_id, role, content, timestamp, updated_at
         ) VALUES (?, ?, ?, ?, 'user', ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(compressed)
    .bind(timestamp)
    .bind(timestamp)
    .execute(pool)
    .await
    .expect("插入测试消息");
    sqlx::query(
        "INSERT INTO render_cache(owner_type, owner_id, topic_id, msg_id, render_content)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(Vec::<u8>::new())
    .execute(pool)
    .await
    .expect("插入测试渲染缓存");
    sqlx::query(
        "INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(message_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .execute(pool)
    .await
    .expect("插入测试搜索索引");
    if active {
        sqlx::query(
            "INSERT INTO active_generations(
                owner_type, owner_id, topic_id, msg_id, created_at
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(message_id)
        .bind(timestamp)
        .execute(pool)
        .await
        .expect("插入测试活动生成");
    }
}

fn edited_anchor_message(content: &str) -> ChatMessage {
    ChatMessage {
        id: "same-message".to_string(),
        role: "user".to_string(),
        content: content.to_string(),
        timestamp: 100,
        topic_id: Some("edit-topic".to_string()),
        ..Default::default()
    }
}

fn edited_anchor_message_with_attachment(
    content: &str,
    topic_id: &str,
    hash: &str,
    size: u64,
) -> ChatMessage {
    ChatMessage {
        attachments: Some(vec![Attachment {
            r#type: "text/plain".to_string(),
            name: "replacement.txt".to_string(),
            size,
            hash: Some(hash.to_string()),
            attachment_order: Some(2),
            ..Default::default()
        }]),
        topic_id: Some(topic_id.to_string()),
        ..edited_anchor_message(content)
    }
}

struct ManagedTestAttachment {
    hash: String,
    path: PathBuf,
    size: u64,
}

fn write_managed_test_attachment(
    app_handle: &tauri::AppHandle<tauri::test::MockRuntime>,
    bytes: &[u8],
    suffix: &str,
) -> ManagedTestAttachment {
    let root = get_attachments_root_dir(app_handle).expect("resolve managed attachment root");
    fs::create_dir_all(&root).expect("create managed attachment root");
    let hash = calculate_sha256(bytes);
    let path = root.join(format!("{hash}.{suffix}"));
    fs::write(&path, bytes).expect("write managed attachment fixture");
    ManagedTestAttachment {
        hash,
        path,
        size: bytes.len() as u64,
    }
}

async fn register_managed_test_attachment(
    pool: &sqlx::SqlitePool,
    attachment: &ManagedTestAttachment,
) {
    sqlx::query(
        "INSERT INTO attachments(hash, mime_type, size, internal_path, created_at)
         VALUES (?, 'text/plain', ?, ?, 100)",
    )
    .bind(&attachment.hash)
    .bind(attachment.size as i64)
    .bind(attachment.path.to_string_lossy().as_ref())
    .execute(pool)
    .await
    .expect("register managed attachment fixture");
}

async fn seed_edit_history_with_anchor_attachment(
    pool: &sqlx::SqlitePool,
    key: &TopicKey,
    original_attachment: &ManagedTestAttachment,
) {
    insert_message(pool, key, "anchor body", true).await;
    sqlx::query(
        "UPDATE attachments
         SET hash = ?, size = ?, internal_path = ?, mime_type = 'text/plain'
         WHERE hash = 'hash-a'",
    )
    .bind(&original_attachment.hash)
    .bind(original_attachment.size as i64)
    .bind(original_attachment.path.to_string_lossy().as_ref())
    .execute(pool)
    .await
    .expect("replace original managed attachment metadata");
    sqlx::query(
        "UPDATE message_attachments SET hash = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id = 'same-message' AND hash = 'hash-a'",
    )
    .bind(&original_attachment.hash)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(pool)
    .await
    .expect("replace original managed attachment relation");
    insert_extra_message(pool, key, "tail-a", 200, false).await;
    insert_extra_message(pool, key, "tail-z", 201, true).await;
    sqlx::query(
        "UPDATE topics SET msg_count = 3
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(pool)
    .await
    .expect("初始化带附件编辑话题计数");
}

async fn seed_edit_history(pool: &sqlx::SqlitePool, key: &TopicKey) {
    insert_message(pool, key, "anchor body", false).await;
    insert_extra_message(pool, key, "tail-a", 200, false).await;
    insert_extra_message(pool, key, "tail-z", 201, true).await;
    sqlx::query(
        "UPDATE topics SET msg_count = 3
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(pool)
    .await
    .expect("初始化编辑话题计数");
}

async fn read_message_body(pool: &sqlx::SqlitePool, key: &TopicKey, message_id: &str) -> String {
    let compressed: Vec<u8> = sqlx::query_scalar(
        "SELECT content FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(message_id)
    .fetch_one(pool)
    .await
    .expect("读取消息正文");
    String::from_utf8(zstd::bulk::decompress(&compressed, 4096).expect("解压消息正文"))
        .expect("消息正文 UTF-8")
}

async fn read_topic_and_owner_hashes(pool: &sqlx::SqlitePool, key: &TopicKey) -> (String, String) {
    match key.owner_type.as_str() {
        "agent" => sqlx::query_as(
            "SELECT t.content_hash, a.content_hash
             FROM topics t JOIN agents a ON a.agent_id = t.owner_id
             WHERE t.owner_type = ? AND t.owner_id = ? AND t.topic_id = ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(pool)
        .await
        .expect("读取话题和代理哈希"),
        other => panic!("不支持的测试 owner 类型 {other}"),
    }
}

#[tokio::test]
async fn batch_loader_keeps_same_topic_and_message_id_isolated() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", true).await;
    insert_message(&pool, &group, "group body", false).await;
    let loaded = load_multi_topic_messages_for_keys(&pool, &[agent.clone(), group.clone()])
        .await
        .expect("load owner-scoped messages");
    assert_eq!(loaded[&agent][0].content, "agent body");
    assert_eq!(loaded[&agent][0].updated_at, Some(101));
    assert_eq!(
        loaded[&agent][0].attachments.as_ref().unwrap()[0].attachment_order,
        Some(7)
    );
    assert_eq!(loaded[&group][0].content, "group body");
    assert!(loaded[&group][0].attachments.is_none());
}

#[tokio::test]
async fn owner_scoped_delete_does_not_touch_other_owner() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", true).await;
    insert_message(&pool, &group, "group body", false).await;
    sqlx::query(
        "UPDATE messages SET deleted_at = 9000000000000
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .execute(&pool)
    .await
    .expect("seed future tombstone");
    let result = delete_messages_for_topic(&pool, &agent, vec!["same-message".to_string()])
        .await
        .expect("delete agent message");
    assert_eq!(result.msg_count, 0);
    let agent_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load agent tombstone");
    assert_eq!(agent_deleted, Some(9000000000000));
    let group_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'owner-g' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load group message");
    assert!(group_deleted.is_none());
    let group_fts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages_fts
         WHERE owner_type = 'group' AND owner_id = 'owner-g'",
    )
    .fetch_one(&pool)
    .await
    .expect("count group fts");
    assert_eq!(group_fts, 1);
    let agent_attachments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_attachments
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("count deleted attachments");
    assert_eq!(agent_attachments, 0);
    let (topic_hash, owner_hash): (String, String) = sqlx::query_as(
        "SELECT t.content_hash, a.content_hash
         FROM topics t JOIN agents a ON a.agent_id = t.owner_id
         WHERE t.owner_type = 'agent' AND t.owner_id = 'owner-a'
           AND t.topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("read bubbled hashes");
    assert!(topic_hash.is_empty());
    assert!(owner_hash.len() == 64 && owner_hash != "before");
}

#[tokio::test]
async fn 删除未读消息回退计数并拒绝迟到未读记账() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "unread-delete-topic");
    insert_message(&pool, &key, "message body", false).await;
    sqlx::query(
        "UPDATE topics SET unread = 1, unread_count = 1
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&pool)
    .await
    .expect("seed unread topic count");
    sqlx::query(
        "INSERT INTO message_unread_receipts(
            owner_type, owner_id, topic_id, msg_id, created_at, counted_unread
         ) VALUES (?, ?, ?, 'same-message', 1, 1)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&pool)
    .await
    .expect("seed unread receipt");

    let result = delete_messages_for_topic(&pool, &key, vec!["same-message".to_string()])
        .await
        .expect("delete live unread message");
    assert_eq!(result.msg_count, 0);
    let state: (i32, i32) = sqlx::query_as(
        "SELECT unread, unread_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("read corrected topic unread state");
    assert_eq!(state, (0, 0));
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("count deleted unread receipts");
    assert_eq!(receipt_count, 0);

    let late = record_topic_unread_for_message_in_pool(&pool, &key, "same-message", true, 2).await;
    assert!(
        late.is_err(),
        "deleted messages must reject late unread events"
    );
    let final_count: i32 = sqlx::query_scalar(
        "SELECT unread_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("read final unread count");
    assert_eq!(final_count, 0);
}

#[tokio::test]
async fn 标记已读后删除旧消息不会回退新消息未读计数() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "read-then-delete-topic");
    insert_message(&pool, &key, "message body", false).await;
    insert_extra_message(&pool, &key, "message-b", 200, false).await;

    record_topic_unread_for_message_in_pool(&pool, &key, "same-message", true, 1)
        .await
        .expect("记录旧消息未读收据");
    set_topic_unread_in_pool(&pool, &key, false, 2)
        .await
        .expect("标记话题已读");
    record_topic_unread_for_message_in_pool(&pool, &key, "message-b", true, 3)
        .await
        .expect("记录新消息未读收据");

    delete_messages_for_topic(&pool, &key, vec!["same-message".to_string()])
        .await
        .expect("删除已读旧消息");

    let unread_count: i32 = sqlx::query_scalar(
        "SELECT unread_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("读取新消息未读计数");
    assert_eq!(unread_count, 1);
    let counted_old_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_unread_receipts
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id = 'same-message' AND counted_unread = 1",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("读取旧消息未读收据");
    assert_eq!(counted_old_receipts, 0);
}

#[tokio::test]
async fn legacy_batch_loader_rejects_ambiguous_topic_id() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", false).await;
    insert_message(&pool, &group, "group body", false).await;
    let error = load_multi_topic_messages(&pool, &["shared-topic".to_string()])
        .await
        .expect_err("legacy loader must fail closed");
    assert!(error.contains("ambiguous"));
}

#[tokio::test]
async fn owner_scoped_truncate_keeps_other_owner_history() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", false).await;
    insert_message(&pool, &group, "group body", false).await;
    sqlx::query(
        "UPDATE messages SET timestamp = 200
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .execute(&pool)
    .await
    .expect("raise agent message timestamp");
    truncate_history_after_timestamp_for_topic(&pool, &agent, "same-message", true)
        .await
        .expect("truncate agent history");
    let agent_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'agent' AND owner_id = 'owner-a' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load agent tombstone");
    assert!(agent_deleted.is_some());
    let group_deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = 'group' AND owner_id = 'owner-g' AND topic_id = 'shared-topic'",
    )
    .fetch_one(&pool)
    .await
    .expect("load group message");
    assert!(group_deleted.is_none());
    let group_fts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages_fts
         WHERE owner_type = 'group' AND owner_id = 'owner-g'",
    )
    .fetch_one(&pool)
    .await
    .expect("count group fts");
    assert_eq!(group_fts, 1);
}

#[tokio::test]
async fn 稳定锚点按同毫秒消息全序处理包含和不包含锚点() {
    let agent = TopicKey::new("agent", "owner-a", "ordered-topic");
    for include_anchor in [false, true] {
        let pool = test_pool().await;
        insert_message(&pool, &agent, "anchor body", false).await;
        insert_extra_message(&pool, &agent, "a-before", 100, false).await;
        insert_extra_message(&pool, &agent, "z-after", 100, true).await;
        let result = truncate_history_after_timestamp_for_topic(
            &pool,
            &agent,
            "same-message",
            include_anchor,
        )
        .await
        .expect("按稳定锚点截断");
        let expected_deleted = if include_anchor {
            vec!["same-message".to_string(), "z-after".to_string()]
        } else {
            vec!["z-after".to_string()]
        };
        assert_eq!(result.deleted_ids, expected_deleted);
        assert_eq!(result.msg_count, if include_anchor { 1 } else { 2 });
        assert_eq!(result.active_ids, vec!["z-after".to_string()]);
        assert_eq!(result.anchor.unwrap().include_anchor, include_anchor);
    }
}

#[tokio::test]
async fn 稳定锚点缺失已删和跨_owner均安全失败() {
    let pool = test_pool().await;
    let agent = TopicKey::new("agent", "owner-a", "shared-topic");
    let group = TopicKey::new("group", "owner-g", "shared-topic");
    insert_message(&pool, &agent, "agent body", false).await;
    insert_message(&pool, &group, "group body", false).await;
    assert!(
        truncate_history_after_timestamp_for_topic(&pool, &agent, "missing", true)
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE messages SET deleted_at = 123
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'
           AND topic_id = 'shared-topic' AND msg_id = 'same-message'",
    )
    .execute(&pool)
    .await
    .expect("标记测试消息删除");
    assert!(
        truncate_history_after_timestamp_for_topic(&pool, &agent, "same-message", true)
            .await
            .is_err()
    );
    insert_extra_message(&pool, &group, "group-anchor", 100, false).await;
    assert!(
        truncate_history_after_timestamp_for_topic(&pool, &agent, "group-anchor", true)
            .await
            .is_err()
    );
    let missing_edit = ChatMessage {
        id: "missing".to_string(),
        role: "user".to_string(),
        content: "edited".to_string(),
        topic_id: Some(agent.topic_id.clone()),
        ..Default::default()
    };
    assert!(edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &agent.owner_id,
        &agent.owner_type,
        agent.topic_id.clone(),
        "missing".to_string(),
        missing_edit,
    )
    .await
    .is_err());
    let cross_owner_edit = ChatMessage {
        id: "group-anchor".to_string(),
        role: "user".to_string(),
        content: "edited".to_string(),
        topic_id: Some(agent.topic_id.clone()),
        ..Default::default()
    };
    assert!(edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &agent.owner_id,
        &agent.owner_type,
        agent.topic_id.clone(),
        "group-anchor".to_string(),
        cross_owner_edit,
    )
    .await
    .is_err());
}

async fn assert_edit_history_unchanged(pool: &sqlx::SqlitePool, key: &TopicKey) {
    assert_eq!(
        read_message_body(pool, key, "same-message").await,
        "anchor body"
    );
    for message_id in ["tail-a", "tail-z"] {
        let deleted_at: Option<i64> = sqlx::query_scalar(
            "SELECT deleted_at FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .bind(message_id)
        .fetch_one(pool)
        .await
        .expect("读取编辑尾部删除状态");
        assert_eq!(deleted_at, None, "{message_id} 应保持活动");
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT msg_count FROM topics
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .expect("读取编辑话题计数");
    assert_eq!(count, 3);
    for table in ["render_cache", "messages_fts"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
        ))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(pool)
        .await
        .expect("读取编辑附属数据");
        assert_eq!(count, 3, "{table} 应保持完整");
    }
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(pool)
    .await
    .expect("读取编辑活动生成");
    assert_eq!(active_count, 1);
    assert_eq!(
        read_topic_and_owner_hashes(pool, key).await,
        ("before".into(), "before".into())
    );
}

#[tokio::test]
async fn 编辑重发在单一事务内更新锚点并截断复合身份尾部() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-topic");
    seed_edit_history(&pool, &key).await;

    let result = edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message("edited body"),
    )
    .await
    .expect("编辑重发 mutation");

    assert_eq!(result.deleted_ids, vec!["tail-a", "tail-z"]);
    assert_eq!(result.active_ids, vec!["tail-z"]);
    assert_eq!(result.msg_count, 1);
    assert_eq!(result.anchor.as_ref().unwrap().include_anchor, false);
    assert!(!result.blocks.is_empty());
    assert_eq!(
        read_message_body(&pool, &key, "same-message").await,
        "edited body"
    );
    let live_tail: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("读取编辑后活动消息");
    assert_eq!(live_tail, 1);
    for table in ["render_cache", "messages_fts"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table}
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
        ))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&pool)
        .await
        .expect("读取编辑后附属数据");
        assert_eq!(count, 1, "{table} 应仅保留锚点");
    }
    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM active_generations
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .expect("读取编辑后活动生成");
    assert_eq!(active_count, 0);
    assert_ne!(
        read_topic_and_owner_hashes(&pool, &key).await,
        ("before".into(), "before".into())
    );
}

#[tokio::test]
async fn 编辑重发替换锚点附件关系并更新附件实体() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-attachment-topic");
    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool: pool.clone(),
        path: PathBuf::from("test.sqlite"),
    });
    let original = write_managed_test_attachment(
        app.handle(),
        b"original attachment bytes for replacement success",
        "txt",
    );
    let replacement = write_managed_test_attachment(
        app.handle(),
        b"replacement attachment bytes for replacement success",
        "txt",
    );
    seed_edit_history_with_anchor_attachment(&pool, &key, &original).await;
    register_managed_test_attachment(&pool, &replacement).await;

    edit_message_and_truncate_history(
        app.handle().clone(),
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message_with_attachment(
            "edited body with replacement",
            &key.topic_id,
            &replacement.hash,
            replacement.size,
        ),
    )
    .await
    .expect("编辑重发附件替换");

    let relation: (String, i64) = sqlx::query_as(
        "SELECT hash, attachment_order FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind("same-message")
    .fetch_one(&pool)
    .await
    .expect("读取替换后的附件关系");
    assert_eq!(relation, (replacement.hash.clone(), 2));
    let attachment_updated_at: i64 =
        sqlx::query_scalar("SELECT updated_at FROM attachments WHERE hash = ?")
            .bind(&replacement.hash)
            .fetch_one(&pool)
            .await
            .expect("读取替换后的附件实体");
    assert!(attachment_updated_at > 0);
    assert!(replacement.path.exists());
    assert!(original.path.exists());
}

#[tokio::test]
async fn 编辑重发锚点写入失败时全部回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-topic");
    seed_edit_history(&pool, &key).await;
    sqlx::query(
        "CREATE TRIGGER fail_edit_anchor
         BEFORE UPDATE OF content ON messages
         WHEN OLD.msg_id = 'same-message'
         BEGIN SELECT RAISE(ABORT, 'injected edit anchor failure'); END",
    )
    .execute(&pool)
    .await
    .expect("创建锚点故障注入");

    let error = edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message("edited body"),
    )
    .await
    .expect_err("锚点写入失败应回滚");
    assert!(error.contains("injected edit anchor failure"), "{error}");
    assert_edit_history_unchanged(&pool, &key).await;
}

#[tokio::test]
async fn 编辑重发哈希刷新失败时锚点尾部计数和哈希全部回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-topic");
    seed_edit_history(&pool, &key).await;
    sqlx::query(
        "CREATE TRIGGER fail_edit_hash
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected edit hash failure'); END",
    )
    .execute(&pool)
    .await
    .expect("创建哈希故障注入");

    let error = edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message("edited body"),
    )
    .await
    .expect_err("哈希刷新失败应回滚");
    assert!(error.contains("injected edit hash failure"), "{error}");
    assert_edit_history_unchanged(&pool, &key).await;
}

#[tokio::test]
async fn 编辑重发附件替换失败时关系和附件实体全部回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-attachment-rollback-topic");
    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool: pool.clone(),
        path: PathBuf::from("test.sqlite"),
    });
    let original = write_managed_test_attachment(
        app.handle(),
        b"original attachment bytes for replacement rollback",
        "txt",
    );
    let replacement = write_managed_test_attachment(
        app.handle(),
        b"replacement attachment bytes for replacement rollback",
        "txt",
    );
    seed_edit_history_with_anchor_attachment(&pool, &key, &original).await;
    register_managed_test_attachment(&pool, &replacement).await;
    sqlx::query(
        "CREATE TRIGGER fail_edit_attachment_hash
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected edit attachment hash failure'); END",
    )
    .execute(&pool)
    .await
    .expect("创建附件编辑哈希故障注入");

    let error = edit_message_and_truncate_history(
        app.handle().clone(),
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message_with_attachment(
            "edited body with replacement",
            &key.topic_id,
            &replacement.hash,
            replacement.size,
        ),
    )
    .await
    .expect_err("附件替换哈希失败应回滚");
    assert!(
        error.contains("injected edit attachment hash failure"),
        "{error}"
    );

    let relation: (String, i64) = sqlx::query_as(
        "SELECT hash, attachment_order FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind("same-message")
    .fetch_one(&pool)
    .await
    .expect("读取回滚后的原附件关系");
    assert_eq!(relation, (original.hash.clone(), 7));
    let replacement_relation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ? AND hash = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind("same-message")
    .bind(&replacement.hash)
    .fetch_one(&pool)
    .await
    .expect("读取回滚后的替换附件关系");
    assert_eq!(replacement_relation_count, 0);
    let replacement_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash = ?")
            .bind(&replacement.hash)
            .fetch_one(&pool)
            .await
            .expect("读取回滚后的替换附件实体");
    // The replacement CAS is pre-registered and verified by
    // ensure_attachments_locally before the transaction.  Rolling back the
    // edit must preserve that standalone CAS while restoring the old relation.
    assert_eq!(replacement_count, 1);
    assert!(replacement.path.exists());
    assert_edit_history_unchanged(&pool, &key).await;
}

#[tokio::test]
async fn 编辑重发尾部清理失败时锚点和所有派生数据全部回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "edit-topic");
    seed_edit_history(&pool, &key).await;
    sqlx::query(
        "CREATE TRIGGER fail_edit_tail
         BEFORE UPDATE OF deleted_at ON messages
         WHEN OLD.msg_id = 'tail-a'
         BEGIN SELECT RAISE(ABORT, 'injected edit tail failure'); END",
    )
    .execute(&pool)
    .await
    .expect("创建尾部故障注入");

    let error = edit_message_and_truncate_history_with_loaded_attachments(
        &pool,
        &key.owner_id,
        &key.owner_type,
        key.topic_id.clone(),
        "same-message".to_string(),
        edited_anchor_message("edited body"),
    )
    .await
    .expect_err("尾部清理失败应回滚");
    assert!(error.contains("injected edit tail failure"), "{error}");
    assert_edit_history_unchanged(&pool, &key).await;
}

#[tokio::test]
async fn 稳定锚点事务失败时消息和附属数据全部回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "rollback-topic");
    insert_message(&pool, &key, "rollback body", true).await;
    sqlx::query(
        "UPDATE topics SET deleted_at = 1
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&pool)
    .await
    .expect("标记测试话题删除");
    assert!(
        delete_messages_for_topic(&pool, &key, vec!["same-message".to_string()])
            .await
            .is_err()
    );
    let deleted: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM messages
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind("same-message")
    .fetch_one(&pool)
    .await
    .expect("读取回滚消息");
    assert_eq!(deleted, None);
    for table in ["render_cache", "message_attachments", "messages_fts"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE owner_type = ? AND owner_id = ? AND topic_id = ?"
        ))
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_one(&pool)
        .await
        .expect("读取回滚附属数据");
        assert_eq!(count, 1, "{table} 应保持原数据");
    }
}

#[tokio::test]
async fn 附件逻辑删除哈希失败时附件标记回滚() {
    let pool = test_pool().await;
    let key = TopicKey::new("agent", "owner-a", "attachment-topic");
    insert_message(&pool, &key, "attachment body", true).await;
    sqlx::query(
        "CREATE TRIGGER fail_attachment_bubble
         BEFORE UPDATE OF content_hash ON topics
         BEGIN SELECT RAISE(ABORT, 'injected attachment bubble failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = tauri::test::mock_app();
    app.manage(DbState {
        pool: pool.clone(),
        path: PathBuf::from("test.sqlite"),
    });
    let app_handle = app.handle().clone();
    let result = delete_message_attachment_for_key(
        &app_handle,
        &key.owner_type,
        &key.owner_id,
        &key.topic_id,
        "same-message",
        "hash-a",
    )
    .await;
    let error = result.expect_err("附件哈希失败应回滚逻辑删除");
    assert!(
        error.contains("injected attachment bubble failure"),
        "{error}"
    );
    let deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM message_attachments
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND msg_id = 'same-message' AND hash = 'hash-a'",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(deleted_at, None);
    let hashes: (String, String) = sqlx::query_as(
        "SELECT t.content_hash, a.content_hash
         FROM topics t JOIN agents a ON a.agent_id = t.owner_id
         WHERE t.owner_type = ? AND t.owner_id = ? AND t.topic_id = ?",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hashes, ("before".to_string(), "before".to_string()));
}
