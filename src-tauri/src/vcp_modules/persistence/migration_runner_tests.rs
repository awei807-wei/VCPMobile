use super::test_support::*;
use super::*;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;

#[tokio::test]
async fn fresh_install_records_all_migrations_and_is_idempotent() {
    let pool = empty_pool().await;
    run_migrations(&pool)
        .await
        .expect("fresh migrations should apply");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=8).collect::<Vec<_>>()
    );
    assert!(column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert!(column_exists_on_pool(&pool, "render_cache", "renderer_schema_version").await);
    assert!(column_exists_on_pool(&pool, "avatars", "deleted_at").await);
    assert!(column_exists_on_pool(&pool, "topics", "last_message_updated_at").await);
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
        (1_i64..=8).collect::<Vec<_>>()
    );
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
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version IN (6, 7, 8)")
        .execute(&pool)
        .await
        .expect("tracking gap fixture should apply");

    run_migrations(&pool)
        .await
        .expect("schema probes should repair tracking without duplicate ALTER TABLE");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=8).collect::<Vec<_>>()
    );
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
        (1_i64..=8).collect::<Vec<_>>()
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
