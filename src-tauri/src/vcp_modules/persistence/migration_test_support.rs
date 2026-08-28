use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};

use crate::vcp_modules::persistence::message_repository::ContentCompressor;

const LEGACY_MIGRATIONS: [&str; 5] = [
    include_str!("../../../migrations/0001_create_initial_tables.sql"),
    include_str!("../../../migrations/0002_add_deleted_at_to_message_attachments.sql"),
    include_str!("../../../migrations/0003_create_messages_fts.sql"),
    include_str!("../../../migrations/0004_fix_fts_triggers.sql"),
    include_str!("../../../migrations/0005_ensure_active_generations.sql"),
];

pub(super) async fn empty_pool() -> Pool<Sqlite> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory SQLite should open")
}

pub(super) async fn legacy_pool() -> Pool<Sqlite> {
    let pool = empty_pool().await;
    for migration in LEGACY_MIGRATIONS {
        sqlx::raw_sql(migration)
            .execute(&pool)
            .await
            .expect("legacy migration should apply without SQLx tracking");
    }
    pool
}

pub(super) async fn insert_legacy_business_data(pool: &Pool<Sqlite>) {
    sqlx::raw_sql(
        "INSERT INTO agents
            (agent_id, name, model, config_hash, content_hash, updated_at)
         VALUES ('agent-1', 'Agent', 'model', 'agent-config', 'agent-content', 1);
         INSERT INTO groups
            (group_id, name, config_hash, content_hash, updated_at)
         VALUES ('group-1', 'Group', 'group-config', 'group-content', 1);
         INSERT INTO topics
            (topic_id, owner_type, owner_id, title, created_at, updated_at,
             config_hash, content_hash)
         VALUES ('topic-1', 'agent', 'agent-1', 'Topic', 1, 1,
                 'topic-config', 'topic-content');
         INSERT INTO attachments
            (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES ('attachment-hash', 'text/plain', 7, '/fixture', 1, 1);
         INSERT INTO avatars
            (owner_type, owner_id, avatar_hash, mime_type, image_data, updated_at)
         VALUES ('agent', 'agent-1', 'avatar-hash', 'image/png', X'0102', 1);
         INSERT INTO active_generations
            (msg_id, topic_id, owner_id, owner_type, created_at)
         VALUES ('message-1', 'topic-1', 'agent-1', 'agent', 1);",
    )
    .execute(pool)
    .await
    .expect("legacy owner data should insert");

    let compressed =
        ContentCompressor::compress("保留 fork 压缩正文").expect("fixture content should compress");
    sqlx::query(
        "INSERT INTO messages
            (msg_id, topic_id, role, content, timestamp, content_hash, created_at, updated_at)
         VALUES ('message-1', 'topic-1', 'user', ?, 1, 'message-content', 1, 1)",
    )
    .bind(compressed)
    .execute(pool)
    .await
    .expect("legacy compressed message should insert");
    sqlx::raw_sql(
        "INSERT INTO render_cache (topic_id, msg_id, render_content, updated_at)
         VALUES ('topic-1', 'message-1', X'0304', 1);
         INSERT INTO message_attachments
            (topic_id, msg_id, hash, attachment_order, display_name, created_at)
         VALUES ('topic-1', 'message-1', 'attachment-hash', 0, 'fixture.txt', 1);",
    )
    .execute(pool)
    .await
    .expect("legacy message relations should insert");
}

#[derive(Debug, PartialEq)]
pub(super) struct BusinessSnapshot {
    counts: Vec<i64>,
    hashes: Vec<String>,
    message_content: Vec<u8>,
}

pub(super) async fn business_snapshot(pool: &Pool<Sqlite>) -> BusinessSnapshot {
    let mut counts = Vec::new();
    for table in [
        "agents",
        "groups",
        "topics",
        "messages",
        "attachments",
        "message_attachments",
        "avatars",
        "active_generations",
    ] {
        counts.push(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .expect("business count should load"),
        );
    }
    let hashes = vec![
        sqlx::query_scalar("SELECT content_hash FROM agents WHERE agent_id = 'agent-1'")
            .fetch_one(pool)
            .await
            .expect("agent hash should load"),
        sqlx::query_scalar("SELECT content_hash FROM topics WHERE topic_id = 'topic-1'")
            .fetch_one(pool)
            .await
            .expect("topic hash should load"),
        sqlx::query_scalar(
            "SELECT content_hash FROM messages WHERE topic_id = 'topic-1' AND msg_id = 'message-1'",
        )
        .fetch_one(pool)
        .await
        .expect("message hash should load"),
    ];
    let message_content = sqlx::query_scalar(
        "SELECT content FROM messages WHERE topic_id = 'topic-1' AND msg_id = 'message-1'",
    )
    .fetch_one(pool)
    .await
    .expect("message content should load");
    BusinessSnapshot {
        counts,
        hashes,
        message_content,
    }
}

pub(super) async fn column_exists_on_pool(pool: &Pool<Sqlite>, table: &str, column: &str) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name = ?)")
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .expect("column probe should succeed")
}

pub(super) async fn migration_versions(pool: &Pool<Sqlite>) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("migration versions should load")
}
