//! Composite-identity message persistence facade.
//!
//! Rendering, owner-aware upsert and attachment persistence are kept in
//! focused modules. This file owns the stable public type/path used by the
//! rest of the application.

#[path = "message_repository_attachments.rs"]
mod message_repository_attachments;
#[path = "message_repository_render.rs"]
mod message_repository_render;
#[path = "message_repository_support.rs"]
mod message_repository_support;
#[path = "message_repository_upsert.rs"]
mod message_repository_upsert;

#[allow(unused_imports)]
pub use message_repository_render::{
    process_message_content, rebuild_all_pre_renders, ContentCompressor, MessageRenderCompiler,
    RebuildProgress, RENDERER_SCHEMA_VERSION,
};

/// Internal message repository for owner-aware DB operations.
pub struct MessageRepository;

pub(crate) struct ExistingMessageState {
    pub(crate) role: String,
    pub(crate) name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) content: String,
    pub(crate) timestamp: i64,
    pub(crate) is_group_message: bool,
    pub(crate) group_id: Option<String>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) content_hash: String,
    pub(crate) updated_at: i64,
    pub(crate) deleted_at: Option<i64>,
}

#[cfg(test)]
mod composite_identity_tests {
    use super::{MessageRenderCompiler, MessageRepository};
    use crate::vcp_modules::chat_manager::ChatMessage;
    use crate::vcp_modules::topic_types::TopicKey;
    use sqlx::Row;

    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open test database");
        sqlx::raw_sql(
            "CREATE TABLE topics (
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                topic_id TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                deleted_at INTEGER,
                PRIMARY KEY(owner_type, owner_id, topic_id)
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
        .expect("create composite schema");
        pool
    }

    fn message(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "user".to_string(),
            name: None,
            content: content.to_string(),
            timestamp: 100,
            updated_at: None,
            is_thinking: None,
            agent_id: None,
            group_id: None,
            topic_id: None,
            is_group_message: Some(false),
            finish_reason: None,
            attachments: None,
            blocks: None,
            shell: None,
            content_hash: None,
        }
    }

    #[tokio::test]
    async fn message_cache_and_fts_are_isolated_by_owner() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO topics(owner_type, owner_id, topic_id) VALUES
             ('agent', 'owner-a', 'shared-topic'), ('group', 'owner-g', 'shared-topic')",
        )
        .execute(&pool)
        .await
        .expect("insert topics");
        for (key, content, bytes) in [
            (
                TopicKey::new("agent", "owner-a", "shared-topic"),
                "agent body",
                vec![1_u8],
            ),
            (
                TopicKey::new("group", "owner-g", "shared-topic"),
                "group body",
                vec![2_u8],
            ),
        ] {
            let mut tx = pool.begin().await.expect("begin");
            MessageRepository::upsert_message_for_topic(
                &mut tx,
                &message("shared-message", content),
                &key,
                &bytes,
                true,
            )
            .await
            .expect("upsert");
            tx.commit().await.expect("commit");
        }
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE topic_id = 'shared-topic'")
                .fetch_one(&pool)
                .await
                .expect("count");
        let cache_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM render_cache WHERE topic_id = 'shared-topic'")
                .fetch_one(&pool)
                .await
                .expect("cache count");
        let fts_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts WHERE topic_id = 'shared-topic'")
                .fetch_one(&pool)
                .await
                .expect("fts count");
        assert_eq!((count, cache_count, fts_count), (2, 2, 2));
        let rows = sqlx::query(
            "SELECT m.owner_type, m.owner_id, f.content
             FROM messages m JOIN messages_fts f
               ON f.owner_type = m.owner_type AND f.owner_id = m.owner_id
              AND f.topic_id = m.topic_id AND f.msg_id = m.msg_id
             WHERE m.topic_id = ? ORDER BY m.owner_type",
        )
        .bind("shared-topic")
        .fetch_all(&pool)
        .await
        .expect("load fts rows");
        assert_eq!(rows[0].get::<String, _>("content"), "agent body");
        assert_eq!(rows[1].get::<String, _>("content"), "group body");
    }

    #[tokio::test]
    async fn legacy_topic_only_upsert_rejects_ambiguous_identity() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO topics(owner_type, owner_id, topic_id) VALUES
             ('agent', 'owner-a', 'shared-topic'), ('group', 'owner-g', 'shared-topic')",
        )
        .execute(&pool)
        .await
        .expect("insert topics");
        let mut tx = pool.begin().await.expect("begin");
        let error = MessageRepository::upsert_message(
            &mut tx,
            &message("message", "body"),
            "shared-topic",
            &MessageRenderCompiler::serialize(&[]).expect("serialize"),
            true,
        )
        .await
        .expect_err("ambiguous legacy write");
        assert!(error.contains("ambiguous"));
    }

    #[tokio::test]
    async fn attachment_relations_preserve_explicit_order() {
        let pool = test_pool().await;
        sqlx::query(
            "INSERT INTO topics(owner_type, owner_id, topic_id) VALUES
             ('agent', 'owner-a', 'topic')",
        )
        .execute(&pool)
        .await
        .expect("insert topic");
        let mut message = message("message", "body");
        message.attachments = Some(vec![
            crate::vcp_modules::chat_manager::Attachment {
                name: "second".to_string(),
                src: "second-source".to_string(),
                attachment_order: Some(2),
                ..Default::default()
            },
            crate::vcp_modules::chat_manager::Attachment {
                name: "first".to_string(),
                src: "first-source".to_string(),
                attachment_order: Some(0),
                ..Default::default()
            },
        ]);
        let key = TopicKey::new("agent", "owner-a", "topic");
        let mut tx = pool.begin().await.expect("begin");
        MessageRepository::upsert_message_for_topic(&mut tx, &message, &key, &[], true)
            .await
            .expect("upsert attachments");
        tx.commit().await.expect("commit");
        let rows = sqlx::query(
            "SELECT attachment_order, display_name FROM message_attachments
             WHERE owner_type = 'agent' AND owner_id = 'owner-a'
               AND topic_id = 'topic' AND msg_id = 'message'
             ORDER BY attachment_order",
        )
        .fetch_all(&pool)
        .await
        .expect("load attachments");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<i32, _>("attachment_order"), 0);
        assert_eq!(rows[0].get::<String, _>("display_name"), "first");
        assert_eq!(rows[1].get::<i32, _>("attachment_order"), 2);
        assert_eq!(rows[1].get::<String, _>("display_name"), "second");
    }
}
