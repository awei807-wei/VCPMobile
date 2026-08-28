use std::{fs, path::Path};

use sqlx::{sqlite::SqlitePoolOptions, Row};

use super::run_migrations;
use crate::vcp_modules::persistence::message_content_storage::{
    decode_message_content, normalize_legacy_message_content,
};

const COUNT_TABLES: [&str; 8] = [
    "agents",
    "groups",
    "topics",
    "messages",
    "attachments",
    "message_attachments",
    "avatars",
    "active_generations",
];

async fn counts(pool: &sqlx::SqlitePool) -> Vec<i64> {
    let mut result = Vec::new();
    for table in COUNT_TABLES {
        result.push(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .expect("fixture count should load"),
        );
    }
    result
}

async fn logical_fingerprint(pool: &sqlx::SqlitePool) -> Vec<String> {
    let mut result = sqlx::query_scalar::<_, String>(
        "SELECT value FROM (
            SELECT 'agent|' || agent_id || '|' || config_hash || '|' || content_hash AS value
              FROM agents
            UNION ALL
            SELECT 'group|' || group_id || '|' || config_hash || '|' || content_hash
              FROM groups
            UNION ALL
            SELECT 'topic|' || topic_id || '|' || config_hash || '|' || content_hash
              FROM topics
            UNION ALL
            SELECT 'attachment|' || hash || '|' || size || '|' || internal_path
              FROM attachments
            UNION ALL
            SELECT 'message-attachment|' || topic_id || '|' || msg_id || '|' || hash
              FROM message_attachments
            UNION ALL
            SELECT 'avatar|' || owner_type || '|' || owner_id || '|' || avatar_hash
              FROM avatars
            UNION ALL
            SELECT 'generation|' || msg_id || '|' || topic_id || '|' || owner_type || '|' || owner_id
              FROM active_generations
        ) ORDER BY value",
    )
    .fetch_all(pool)
    .await
    .expect("fixture identity fields should load");
    let message_rows = sqlx::query(
        "SELECT topic_id, msg_id, content_hash, content
         FROM messages ORDER BY topic_id, msg_id",
    )
    .fetch_all(pool)
    .await
    .expect("fixture messages should load");
    for row in message_rows {
        result.push(format!(
            "message|{}|{}|{}|{}",
            row.get::<String, _>("topic_id"),
            row.get::<String, _>("msg_id"),
            row.get::<String, _>("content_hash"),
            decode_message_content(&row, "content").expect("fixture content should decode")
        ));
    }
    result
}

fn remove_test_database(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        path.with_extension("sqlite-wal"),
        path.with_extension("sqlite-shm"),
    ] {
        if candidate.exists() {
            fs::remove_file(candidate).expect("temporary fixture should be removable");
        }
    }
}

#[tokio::test]
#[ignore = "requires VCP_LEGACY_DB_FIXTURE"]
async fn migrates_external_fork_database_copy_without_logical_data_loss() {
    let source = std::env::var_os("VCP_LEGACY_DB_FIXTURE")
        .map(std::path::PathBuf::from)
        .expect("VCP_LEGACY_DB_FIXTURE must point to a non-credential test database");
    let target = std::env::temp_dir().join(format!(
        "vcp-mobile-legacy-migration-{}-{}.sqlite",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::copy(&source, &target).expect("legacy fixture should copy before migration");

    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&target);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("copied legacy database should open");
    let counts_before = counts(&pool).await;
    let fingerprint_before = logical_fingerprint(&pool).await;
    assert_eq!(counts_before, vec![1, 1, 2, 3, 1, 1, 1, 1]);

    run_migrations(&pool)
        .await
        .expect("actual fork fixture should migrate");
    assert_eq!(normalize_legacy_message_content(&pool).await.unwrap(), 3);
    run_migrations(&pool)
        .await
        .expect("second startup should not repeat destructive SQL");
    assert_eq!(normalize_legacy_message_content(&pool).await.unwrap(), 0);

    assert_eq!(counts(&pool).await, counts_before);
    assert_eq!(logical_fingerprint(&pool).await, fingerprint_before);
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(&pool)
            .await
            .unwrap(),
        7
    );
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('render_cache') WHERE name = 'content_hash')"
    )
    .fetch_one(&pool)
    .await
    .unwrap());
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('avatars') WHERE name = 'deleted_at')"
    )
    .fetch_one(&pool)
    .await
    .unwrap());

    pool.close().await;
    remove_test_database(&target);
}
