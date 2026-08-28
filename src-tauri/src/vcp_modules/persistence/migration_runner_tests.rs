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
        (1_i64..=7).collect::<Vec<_>>()
    );
    assert!(column_exists_on_pool(&pool, "render_cache", "content_hash").await);
    assert!(column_exists_on_pool(&pool, "render_cache", "renderer_schema_version").await);
    assert!(column_exists_on_pool(&pool, "avatars", "deleted_at").await);

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
        (1_i64..=7).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn missing_tracking_records_converge_without_repeating_alter_table() {
    let pool = empty_pool().await;
    run_migrations(&pool).await.unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version IN (6, 7)")
        .execute(&pool)
        .await
        .expect("tracking gap fixture should apply");

    run_migrations(&pool)
        .await
        .expect("schema probes should repair tracking without duplicate ALTER TABLE");
    assert_eq!(
        migration_versions(&pool).await,
        (1_i64..=7).collect::<Vec<_>>()
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
        (1_i64..=7).collect::<Vec<_>>()
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
    let migration_four = migrator
        .migrations
        .iter()
        .find(|migration| migration.version == 4)
        .unwrap();
    sqlx::query(
        "INSERT INTO _sqlx_migrations
         (version, description, success, checksum, execution_time)
         VALUES (?, ?, 1, ?, 0)",
    )
    .bind(migration_four.version)
    .bind(migration_four.description.as_ref())
    .bind(migration_four.checksum.as_ref())
    .execute(&pool)
    .await
    .expect("migration four should be tracked for the fixture");
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
