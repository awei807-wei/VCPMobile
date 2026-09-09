use super::test_support::*;
use super::*;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::Row;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_MIGRATION_DATABASE: AtomicU64 = AtomicU64::new(1);

#[tokio::test]
async fn fresh_install_records_all_migrations_and_is_idempotent() {
    let pool = empty_pool().await;
    run_migrations(&pool)
        .await
        .expect("fresh migrations should apply");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=15).collect::<Vec<_>>()
    );
    assert!(column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert!(column_exists_on_pool(&pool, "render_cache", "renderer_schema_version").await);
    assert!(column_exists_on_pool(&pool, "avatars", "deleted_at").await);
    assert!(column_exists_on_pool(&pool, "topics", "last_message_updated_at").await);
    assert!(column_exists_on_pool(&pool, "active_generations", "helper_generation").await);
    assert!(table_exists_on_pool(&pool, "group_member_tags")
        .await
        .unwrap());
    assert!(table_exists_on_pool(&pool, "recovery_cleanup_outbox")
        .await
        .unwrap());
    assert!(table_exists_on_pool(&pool, "attachment_gc_unlink_outbox")
        .await
        .unwrap());
    assert!(table_exists_on_pool(&pool, "message_unread_receipts")
        .await
        .unwrap());
    assert!(column_exists_on_pool(&pool, "message_unread_receipts", "counted_unread").await);
    assert!(
        table_exists_on_pool(&pool, "attachment_gc_unlink_live_references")
            .await
            .unwrap()
    );
    for index in [
        "idx_attachments_internal_path",
        "idx_attachments_thumbnail_path",
        "idx_attachments_hash_nocase",
        "idx_message_attachments_hash_nocase",
    ] {
        assert!(
            sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?
                )",
            )
            .bind(index)
            .fetch_one(&pool)
            .await
            .unwrap()
                != 0,
            "bounded GC index {index} should exist",
        );
    }
    assert_eq!(
        primary_key_columns_on_pool(&pool, "topics").await,
        ["owner_type", "owner_id", "topic_id"]
    );
    assert_eq!(
        primary_key_columns_on_pool(&pool, "messages").await,
        ["owner_type", "owner_id", "topic_id", "msg_id"]
    );

    let migrator = sqlx::migrate!("./migrations");
    for migration in migrator.migrations.iter() {
        let checksum: Vec<u8> =
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                .bind(migration.version)
                .fetch_one(&pool)
                .await
                .expect("migration checksum should load");
        assert_eq!(checksum.as_slice(), migration.checksum.as_ref());
    }

    run_migrations(&pool)
        .await
        .expect("repeated startup should be idempotent");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=15).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn bounded_gc_reference_queries_use_their_declared_indexes() {
    let pool = empty_pool().await;
    run_migrations(&pool)
        .await
        .expect("fresh migrations should apply");

    for (sql, expected_index) in [
        (
            "EXPLAIN QUERY PLAN
             SELECT hash FROM attachments
             WHERE hash COLLATE NOCASE IN (?)",
            "idx_attachments_hash_nocase",
        ),
        (
            "EXPLAIN QUERY PLAN
             SELECT hash FROM attachments
             WHERE internal_path IN (?)",
            "idx_attachments_internal_path",
        ),
        (
            "EXPLAIN QUERY PLAN
             SELECT hash FROM attachments
             WHERE thumbnail_path IN (?)",
            "idx_attachments_thumbnail_path",
        ),
        (
            "EXPLAIN QUERY PLAN
             SELECT hash FROM message_attachments
             WHERE hash COLLATE NOCASE IN (?)",
            "idx_message_attachments_hash_nocase",
        ),
    ] {
        let details = sqlx::query(sql)
            .bind("candidate")
            .fetch_all(&pool)
            .await
            .expect("query plan should load")
            .into_iter()
            .map(|row| row.try_get::<String, _>("detail").unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            details.contains(expected_index),
            "query plan should use {expected_index}, got:\n{details}",
        );
    }
}

#[tokio::test]
async fn composite_identity_keeps_same_topic_and_message_ids_isolated() {
    let pool = empty_pool().await;
    run_migrations(&pool).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO topics
            (owner_type, owner_id, topic_id, title, created_at, updated_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 'Agent topic', 1, 1),
            ('group', 'owner-g', 'shared-topic', 'Group topic', 1, 1);
         INSERT INTO messages
            (owner_type, owner_id, topic_id, msg_id, role, content, timestamp,
             created_at, updated_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 'shared-message', 'user', 'agent', 1, 1, 1),
            ('group', 'owner-g', 'shared-topic', 'shared-message', 'user', 'group', 1, 1, 1);
         INSERT INTO attachments
            (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES ('shared-hash', 'text/plain', 1, '/fixture', 1, 1);
         INSERT INTO render_cache
            (owner_type, owner_id, topic_id, msg_id, render_content, updated_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 'shared-message', X'01', 1),
            ('group', 'owner-g', 'shared-topic', 'shared-message', X'02', 1);
         INSERT INTO message_attachments
            (owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
             display_name, created_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 'shared-message', 'shared-hash', 0, 'agent.txt', 1),
            ('group', 'owner-g', 'shared-topic', 'shared-message', 'shared-hash', 0, 'group.txt', 1);
         INSERT INTO active_generations
            (owner_type, owner_id, topic_id, msg_id, created_at)
         VALUES
            ('agent', 'owner-a', 'shared-topic', 'shared-message', 1),
            ('group', 'owner-g', 'shared-topic', 'shared-message', 1);
         INSERT INTO messages_fts(msg_id, topic_id, content, owner_type, owner_id)
         VALUES
            ('shared-message', 'shared-topic', 'agent', 'agent', 'owner-a'),
            ('shared-message', 'shared-topic', 'group', 'group', 'owner-g');",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "UPDATE messages SET deleted_at = 2
         WHERE owner_type = 'agent' AND owner_id = 'owner-a'
           AND topic_id = 'shared-topic' AND msg_id = 'shared-message'",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages
             WHERE topic_id = 'shared-topic' AND msg_id = 'shared-message'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT content FROM messages
             WHERE owner_type = 'group' AND owner_id = 'owner-g'
               AND topic_id = 'shared-topic' AND msg_id = 'shared-message'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "group"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages_fts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT owner_type FROM messages_fts")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "group"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM message_attachments
             WHERE owner_type = 'group' AND owner_id = 'owner-g'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn composite_bridge_rolls_back_all_table_rewrites_on_orphan_data() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    sqlx::query(
        "INSERT INTO messages
            (msg_id, topic_id, role, content, timestamp, created_at, updated_at)
         VALUES ('orphan-message', 'missing-topic', 'user', 'orphan', 1, 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let message_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();

    let error = run_migrations(&pool)
        .await
        .expect_err("orphan data must abort the composite bridge");
    assert!(error.contains("CHECK constraint") || error.contains("0008"));
    assert_eq!(
        primary_key_columns_on_pool(&pool, "topics").await,
        ["topic_id"]
    );
    assert!(!column_exists_on_pool(&pool, "messages", "owner_type").await);
    assert!(!table_exists_on_pool(&pool, "messages_legacy_wire14")
        .await
        .unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap(),
        message_count_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1,
        "a failed migration must roll back the active-generation cleanup"
    );
    assert!(!migration_versions(&pool).await.contains(&8));
}

#[tokio::test]
async fn composite_bridge_preserves_orphan_active_generations_without_losing_data() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    sqlx::raw_sql(
        "INSERT INTO active_generations
            (msg_id, topic_id, owner_id, owner_type, created_at)
         VALUES
            ('missing-topic-message', 'missing-topic', 'agent-1', 'agent', 2),
            ('missing-message', 'topic-1', 'agent-1', 'agent', 3);
         INSERT INTO messages
            (msg_id, topic_id, role, content, timestamp, created_at, updated_at, deleted_at)
         VALUES ('deleted-message', 'topic-1', 'user', 'preserve this', 1, 1, 1, 4);
         INSERT INTO active_generations
            (msg_id, topic_id, owner_id, owner_type, created_at)
         VALUES ('deleted-message', 'topic-1', 'agent-1', 'agent', 5);",
    )
    .execute(&pool)
    .await
    .unwrap();

    run_migrations(&pool)
        .await
        .expect("recovery metadata must migrate without being discarded");

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        4,
        "every active generation row must be preserved"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM active_generations
             WHERE owner_type = 'agent' AND owner_id = 'agent-1'
               AND topic_id = 'topic-1' AND msg_id = 'message-1' AND created_at = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    for (msg_id, topic_id, created_at) in [
        ("missing-topic-message", "missing-topic", 2_i64),
        ("missing-message", "topic-1", 3_i64),
        ("deleted-message", "topic-1", 5_i64),
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM active_generations
                 WHERE owner_type = 'agent' AND owner_id = 'agent-1'
                   AND topic_id = ? AND msg_id = ? AND created_at = ?",
            )
            .bind(topic_id)
            .bind(msg_id)
            .bind(created_at)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1,
            "active generation rows must retain their complete identity",
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages WHERE msg_id IN
                ('message-1', 'deleted-message')",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2,
        "migrating recovery metadata must not delete user messages"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE msg_id = 'deleted-message' AND deleted_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert!(migration_versions(&pool).await.contains(&8));

    run_migrations(&pool)
        .await
        .expect("a repaired database must not remain blocked on the next startup");
}

#[tokio::test]
async fn composite_bridge_rejects_unmappable_active_generations_without_deletion() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    sqlx::query(
        "INSERT INTO active_generations
            (msg_id, topic_id, owner_id, owner_type, created_at)
         VALUES ('invalid-owner-type', 'topic-1', 'agent-1', 'user', 2)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = run_migrations(&pool)
        .await
        .expect_err("an active generation without a Wire owner type must abort migration");
    assert!(error.contains("CHECK constraint") || error.contains("0008"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM active_generations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2,
        "a failed migration must preserve every legacy recovery row",
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM active_generations
             WHERE owner_type = 'user' AND owner_id = 'agent-1'
               AND topic_id = 'topic-1' AND msg_id = 'invalid-owner-type'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1,
        "failed validation must not silently delete the invalid row",
    );
    assert_eq!(
        primary_key_columns_on_pool(&pool, "active_generations").await,
        ["msg_id"],
    );
    assert!(!migration_versions(&pool).await.contains(&8));
}

#[tokio::test]
async fn missing_tracking_records_converge_without_repeating_alter_table() {
    let pool = empty_pool().await;
    run_migrations(&pool).await.unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version BETWEEN 6 AND 12")
        .execute(&pool)
        .await
        .expect("tracking gap fixture should apply");

    run_migrations(&pool)
        .await
        .expect("schema probes should repair tracking without duplicate ALTER TABLE");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=15).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn partial_group_member_tags_schema_fails_closed_before_migration() {
    let pool = legacy_pool().await;
    sqlx::query(
        "CREATE TABLE group_member_tags (
            group_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            member_tag TEXT NOT NULL,
            PRIMARY KEY (group_id, agent_id)
        )",
    )
    .execute(&pool)
    .await
    .expect("partial group_member_tags fixture should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("partial group_member_tags schema must fail closed");
    assert!(error.contains("group_member_tags"));
    assert!(!table_exists_on_pool(&pool, "_sqlx_migrations")
        .await
        .unwrap());
}

#[tokio::test]
async fn partial_recovery_cleanup_outbox_schema_fails_closed_before_migration() {
    let pool = legacy_pool().await;
    sqlx::query(
        "CREATE TABLE recovery_cleanup_outbox (
            claimed_path TEXT PRIMARY KEY,
            helper_generation BIGINT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .expect("partial recovery outbox fixture should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("partial recovery outbox schema must fail closed");
    assert!(error.contains("outbox"));
    assert!(!table_exists_on_pool(&pool, "_sqlx_migrations")
        .await
        .unwrap());
}

#[tokio::test]
async fn partial_attachment_gc_unlink_outbox_schema_fails_closed_before_migration() {
    let pool = legacy_pool().await;
    sqlx::query(
        "CREATE TABLE attachment_gc_unlink_outbox (
            root_kind TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            PRIMARY KEY (root_kind, relative_path)
        )",
    )
    .execute(&pool)
    .await
    .expect("partial attachment GC outbox fixture should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("partial attachment GC outbox schema must fail closed");
    assert!(error.contains("attachment GC unlink outbox"));
    assert!(!table_exists_on_pool(&pool, "_sqlx_migrations")
        .await
        .unwrap());
}

#[tokio::test]
async fn partial_bounded_gc_reference_indexes_fail_closed_before_migration() {
    let pool = legacy_pool().await;
    sqlx::raw_sql(
        "CREATE INDEX idx_attachments_internal_path ON attachments(internal_path);
         CREATE INDEX idx_attachments_thumbnail_path ON attachments(thumbnail_path);",
    )
    .execute(&pool)
    .await
    .expect("partial bounded GC indexes should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("partial bounded GC indexes must fail closed");
    assert!(error.contains("bounded attachment GC reference-index"));
    assert!(!table_exists_on_pool(&pool, "_sqlx_migrations")
        .await
        .unwrap());
}

#[tokio::test]
async fn tracked_bounded_gc_index_with_wrong_columns_fails_closed() {
    let pool = empty_pool().await;
    run_migrations(&pool)
        .await
        .expect("fresh migrations should apply");
    sqlx::query("DROP INDEX idx_attachments_hash_nocase")
        .execute(&pool)
        .await
        .expect("hash index should drop for mismatch fixture");
    sqlx::query("CREATE INDEX idx_attachments_hash_nocase ON attachments(internal_path)")
        .execute(&pool)
        .await
        .expect("wrong-column hash index should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("tracked wrong-column GC index must fail closed");
    assert!(error.contains("partially applied") || error.contains("v13"));
}

#[tokio::test]
async fn tracked_helper_generation_without_column_fails_closed() {
    let pool = legacy_pool().await;
    let migrator = sqlx::migrate!("./migrations");
    bootstrap_legacy_if_needed(&pool, &migrator)
        .await
        .expect("legacy tracking should bootstrap");
    let migration = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == 10)
        .expect("helper generation migration should exist");
    insert_tracking_record(&pool, migration).await;

    let error = run_migrations(&pool)
        .await
        .expect_err("tracked helper generation without column must fail closed");
    assert!(error.contains("v10") || error.contains("schema invariant"));
    assert!(!column_exists_on_pool(&pool, "active_generations", "helper_generation").await);
}

#[tokio::test]
async fn tracked_cleanup_outbox_without_table_fails_closed() {
    let pool = legacy_pool().await;
    let migrator = sqlx::migrate!("./migrations");
    bootstrap_legacy_if_needed(&pool, &migrator)
        .await
        .expect("legacy tracking should bootstrap");
    let migration = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == 11)
        .expect("cleanup outbox migration should exist");
    insert_tracking_record(&pool, migration).await;

    let error = run_migrations(&pool)
        .await
        .expect_err("tracked cleanup outbox without table must fail closed");
    assert!(error.contains("v11") || error.contains("schema invariant"));
    assert!(!table_exists_on_pool(&pool, "recovery_cleanup_outbox")
        .await
        .unwrap());
}

#[tokio::test]
async fn tracked_attachment_gc_outbox_without_table_fails_closed() {
    let pool = legacy_pool().await;
    let migrator = sqlx::migrate!("./migrations");
    bootstrap_legacy_if_needed(&pool, &migrator)
        .await
        .expect("legacy tracking should bootstrap");
    let migration = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == 12)
        .expect("attachment GC outbox migration should exist");
    insert_tracking_record(&pool, migration).await;

    let error = run_migrations(&pool)
        .await
        .expect_err("tracked attachment GC outbox without table must fail closed");
    assert!(error.contains("v12") || error.contains("schema invariant"));
    assert!(!table_exists_on_pool(&pool, "attachment_gc_unlink_outbox")
        .await
        .unwrap());
}

async fn insert_tracking_record(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    migration: &sqlx::migrate::Migration,
) {
    sqlx::query(
        "INSERT INTO _sqlx_migrations
         (version, description, success, checksum, execution_time)
         VALUES (?, ?, 1, ?, 0)",
    )
    .bind(migration.version)
    .bind(migration.description.as_ref())
    .bind(migration.checksum.as_ref())
    .execute(pool)
    .await
    .expect("tracking fixture should insert");
}

#[tokio::test]
async fn two_pools_startup_converges_to_one_complete_migration_chain() {
    let suffix = NEXT_MIGRATION_DATABASE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "vcp-migration-concurrent-{}-{suffix}.sqlite",
        std::process::id()
    ));
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5));
    let pool_a = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .expect("first migration pool should open");
    let pool_b = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("second migration pool should open");

    let (result_a, result_b) = tokio::join!(run_migrations(&pool_a), run_migrations(&pool_b));
    assert!(
        result_a.is_ok(),
        "first pool migration failed: {result_a:?}"
    );
    assert!(
        result_b.is_ok(),
        "second pool migration failed: {result_b:?}"
    );
    assert_eq!(
        migration_versions(&pool_a).await,
        (1_i64..=15).collect::<Vec<_>>()
    );
    assert!(column_exists_on_pool(&pool_b, "active_generations", "helper_generation").await);
    assert!(table_exists_on_pool(&pool_b, "recovery_cleanup_outbox")
        .await
        .unwrap());

    pool_a.close().await;
    pool_b.close().await;
    std::fs::remove_file(&path).expect("concurrent migration fixture should be removable");
}

#[tokio::test]
async fn legacy_upgrade_preserves_counts_hashes_and_compressed_content() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    let before = business_snapshot(&pool).await;

    run_migrations(&pool)
        .await
        .expect("legacy database should upgrade");
    run_migrations(&pool)
        .await
        .expect("legacy database should remain idempotent");

    assert_eq!(business_snapshot(&pool).await, before);
    let render_identity: (String, i64) = sqlx::query_as(
        "SELECT content_hash, renderer_schema_version FROM render_cache
         WHERE topic_id = 'topic-1' AND msg_id = 'message-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("render identity defaults should load");
    assert_eq!(render_identity, (String::new(), 0));
    let avatar_deleted_at: Option<i64> = sqlx::query_scalar(
        "SELECT deleted_at FROM avatars WHERE owner_type = 'agent' AND owner_id = 'agent-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("avatar tombstone should load");
    assert_eq!(avatar_deleted_at, None);

    let row = sqlx::query(
        "SELECT content FROM messages WHERE topic_id = 'topic-1' AND msg_id = 'message-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("message content should load");
    assert_eq!(
        decode_message_content(&row, "content").unwrap(),
        "保留 fork 压缩正文"
    );
}

#[tokio::test]
async fn group_member_tags_migration_preserves_removed_member_tags() {
    let pool = legacy_pool().await;
    sqlx::raw_sql(
        "INSERT INTO groups (group_id, name, updated_at)
             VALUES ('group-tags', '标签组', 1);
         INSERT INTO group_members (group_id, agent_id, member_tag, sort_order, updated_at)
             VALUES
             ('group-tags', 'agent-a', '主持人', 0, 2),
             ('group-tags', 'agent-b', '', 1, 2),
             ('group-tags', 'agent-c', NULL, 2, 2);",
    )
    .execute(&pool)
    .await
    .expect("旧成员标签样例写入失败");

    run_migrations(&pool).await.expect("成员标签迁移应成功");
    let tags: Vec<(String, String)> = sqlx::query_as(
        "SELECT agent_id, member_tag FROM group_member_tags
         WHERE group_id = 'group-tags' ORDER BY agent_id",
    )
    .fetch_all(&pool)
    .await
    .expect("读取迁移后的成员标签失败");
    assert_eq!(tags, vec![("agent-a".to_string(), "主持人".to_string())]);

    sqlx::query(
        "UPDATE group_member_tags SET member_tag = '本地新标签'
         WHERE group_id = 'group-tags' AND agent_id = 'agent-a'",
    )
    .execute(&pool)
    .await
    .expect("更新迁移后标签失败");
    sqlx::raw_sql(include_str!(
        "../../../migrations/0009_decouple_group_member_tags.sql"
    ))
    .execute(&pool)
    .await
    .expect("重复执行标签迁移失败");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT member_tag FROM group_member_tags
             WHERE group_id = 'group-tags' AND agent_id = 'agent-a'",
        )
        .fetch_one(&pool)
        .await
        .expect("读取重复迁移后的标签失败"),
        "本地新标签"
    );

    sqlx::query("DELETE FROM group_members WHERE group_id = 'group-tags' AND agent_id = 'agent-a'")
        .execute(&pool)
        .await
        .expect("移除成员失败");
    let retained: String = sqlx::query_scalar(
        "SELECT member_tag FROM group_member_tags
         WHERE group_id = 'group-tags' AND agent_id = 'agent-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("移除成员后标签应保留");
    assert_eq!(retained, "本地新标签");

    sqlx::query(
        "INSERT INTO group_members (group_id, agent_id, sort_order, updated_at)
         VALUES ('group-tags', 'agent-a', 0, 3)",
    )
    .execute(&pool)
    .await
    .expect("重新加入成员失败");
    run_migrations(&pool).await.expect("重复启动应保持迁移幂等");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM group_member_tags WHERE group_id = 'group-tags'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn missing_active_generations_remains_a_pending_repair() {
    let pool = legacy_pool().await;
    sqlx::query("DROP TABLE active_generations")
        .execute(&pool)
        .await
        .expect("repair fixture table should drop");
    run_migrations(&pool)
        .await
        .expect("repair migration should recreate the table");
    assert!(table_exists_on_pool(&pool, "active_generations")
        .await
        .unwrap());
    assert!(migration_versions(&pool).await.contains(&5));
}

#[tokio::test]
async fn interrupted_legacy_seed_rolls_back_and_retries_cleanly() {
    let pool = legacy_pool().await;
    sqlx::query(MIGRATION_TABLE_SQL)
        .execute(&pool)
        .await
        .expect("tracking table should be created");
    sqlx::raw_sql(
        "CREATE TRIGGER fail_second_seed
         BEFORE INSERT ON _sqlx_migrations WHEN NEW.version = 2
         BEGIN SELECT RAISE(ABORT, 'forced seed failure'); END;",
    )
    .execute(&pool)
    .await
    .expect("failure trigger should install");

    let migrator = sqlx::migrate!("./migrations");
    let error = bootstrap_legacy_if_needed(&pool, &migrator)
        .await
        .expect_err("injected seed failure should escape");
    assert!(error.contains("forced seed failure"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    sqlx::query("DROP TRIGGER fail_second_seed")
        .execute(&pool)
        .await
        .expect("failure trigger should drop");
    run_migrations(&pool)
        .await
        .expect("retry should converge after the transient failure");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=15).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn partial_render_migration_fails_closed_without_business_changes() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    let before = business_snapshot(&pool).await;
    sqlx::query(
        "ALTER TABLE render_cache ADD COLUMN renderer_schema_version INTEGER NOT NULL DEFAULT 0",
    )
    .execute(&pool)
    .await
    .expect("partial migration fixture should apply");

    let error = run_migrations(&pool)
        .await
        .expect_err("partial schema must not be guessed or overwritten");
    assert!(error.contains("partially applied"));
    assert!(!column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert_eq!(business_snapshot(&pool).await, before);
    assert!(!table_exists_on_pool(&pool, "_sqlx_migrations")
        .await
        .unwrap());
}

#[tokio::test]
async fn multi_statement_migration_rolls_back_its_first_alter_on_failure() {
    let pool = legacy_pool().await;
    insert_legacy_business_data(&pool).await;
    let before = business_snapshot(&pool).await;
    let migrator = sqlx::migrate!("./migrations");
    bootstrap_legacy_if_needed(&pool, &migrator)
        .await
        .expect("legacy tracking should bootstrap");
    sqlx::query(
        "ALTER TABLE render_cache ADD COLUMN renderer_schema_version INTEGER NOT NULL DEFAULT 0",
    )
    .execute(&pool)
    .await
    .expect("second-column conflict fixture should apply");

    migrator
        .run(&pool)
        .await
        .expect_err("migration 6 should fail on its second ALTER TABLE");
    assert!(!column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert_eq!(business_snapshot(&pool).await, before);
    assert!(!migration_versions(&pool).await.contains(&6));
}

#[tokio::test]
async fn mismatched_checksum_fails_before_new_schema_is_applied() {
    let pool = legacy_pool().await;
    sqlx::query(MIGRATION_TABLE_SQL)
        .execute(&pool)
        .await
        .expect("tracking table should be created");
    sqlx::query(
        "INSERT INTO _sqlx_migrations
         (version, description, success, checksum, execution_time)
         VALUES (1, 'create initial tables', 1, X'00', 0)",
    )
    .execute(&pool)
    .await
    .expect("invalid checksum fixture should insert");

    let error = run_migrations(&pool)
        .await
        .expect_err("checksum mismatch must fail closed");
    assert!(error.contains("checksum"));
    assert!(!column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert!(!column_exists_on_pool(&pool, "avatars", "deleted_at").await);
    assert_eq!(migration_versions(&pool).await, vec![1]);
}

#[tokio::test]
async fn dirty_tracking_record_fails_before_new_schema_is_applied() {
    let pool = legacy_pool().await;
    let migrator = sqlx::migrate!("./migrations");
    let migration_one = migrator.migrations.first().unwrap();
    sqlx::query(MIGRATION_TABLE_SQL)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations
         (version, description, success, checksum, execution_time)
         VALUES (?, ?, 0, ?, 0)",
    )
    .bind(migration_one.version)
    .bind(migration_one.description.as_ref())
    .bind(migration_one.checksum.as_ref())
    .execute(&pool)
    .await
    .unwrap();

    let error = run_migrations(&pool)
        .await
        .expect_err("dirty migration state must fail closed");
    assert!(error.contains("dirty"));
    assert!(!column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert!(!column_exists_on_pool(&pool, "avatars", "deleted_at").await);
}
