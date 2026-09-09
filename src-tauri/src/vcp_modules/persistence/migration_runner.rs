use std::collections::HashSet;
use std::sync::OnceLock;

use sqlx::{migrate::Migrator, Pool, Row, Sqlite, Transaction};
use tokio::sync::Mutex;

#[path = "migration_schema.rs"]
mod schema;
use schema::{inspect_legacy_schema, LegacySchema};

const MIGRATION_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version        BIGINT PRIMARY KEY,
    description    TEXT NOT NULL,
    installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success        BOOLEAN NOT NULL,
    checksum       BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)";

static MIGRATION_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), String> {
    let _migration_guard = MIGRATION_MUTEX.get_or_init(|| Mutex::new(())).lock().await;
    let migrator = sqlx::migrate!("./migrations");
    bootstrap_legacy_if_needed(pool, &migrator).await?;
    migrator
        .run(pool)
        .await
        .map_err(|error| format!("数据库初始化失败: {error}"))
}

/// 将无追踪记录的 fork 旧库桥接到 SQLx 迁移链，并修复可证明已完成的中断 seed。
async fn bootstrap_legacy_if_needed(
    pool: &Pool<Sqlite>,
    migrator: &Migrator,
) -> Result<(), String> {
    let has_messages = table_exists_on_pool(pool, "messages").await?;
    let has_tracking = table_exists_on_pool(pool, "_sqlx_migrations").await?;
    if !has_messages {
        reject_tracking_without_baseline(pool, has_tracking).await?;
        return Ok(());
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| format!("Bootstrap: failed to begin transaction: {error}"))?;
    let schema = inspect_legacy_schema(&mut transaction).await?;
    schema.validate_partial_states()?;
    let records = load_tracking_records(&mut transaction, has_tracking).await?;
    validate_tracking_records(&records, migrator, &schema)?;

    sqlx::query(MIGRATION_TABLE_SQL)
        .execute(&mut *transaction)
        .await
        .map_err(|error| format!("Bootstrap: failed to create _sqlx_migrations: {error}"))?;
    seed_proven_migrations(&mut transaction, migrator, &schema, &records).await?;
    transaction
        .commit()
        .await
        .map_err(|error| format!("Bootstrap: failed to commit migration bridge: {error}"))?;
    Ok(())
}

async fn table_exists_on_pool(pool: &Pool<Sqlite>, table: &str) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect table {table}: {error}"))
}

async fn reject_tracking_without_baseline(
    pool: &Pool<Sqlite>,
    has_tracking: bool,
) -> Result<(), String> {
    if !has_tracking {
        return Ok(());
    }
    let record_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await
        .map_err(|error| format!("Bootstrap: failed to inspect migration records: {error}"))?;
    if record_count > 0 {
        return Err(
            "Bootstrap: migration records exist but the messages baseline table is missing"
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Debug)]
struct TrackingRecord {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

async fn load_tracking_records(
    transaction: &mut Transaction<'_, Sqlite>,
    has_tracking: bool,
) -> Result<Vec<TrackingRecord>, String> {
    if !has_tracking {
        return Ok(Vec::new());
    }
    let rows = sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations")
        .fetch_all(&mut **transaction)
        .await
        .map_err(|error| format!("Bootstrap: failed to read migration records: {error}"))?;
    rows.into_iter()
        .map(|row| {
            Ok(TrackingRecord {
                version: row
                    .try_get("version")
                    .map_err(|error| format!("Bootstrap: invalid migration version: {error}"))?,
                success: row
                    .try_get("success")
                    .map_err(|error| format!("Bootstrap: invalid migration status: {error}"))?,
                checksum: row
                    .try_get("checksum")
                    .map_err(|error| format!("Bootstrap: invalid migration checksum: {error}"))?,
            })
        })
        .collect()
}

fn validate_tracking_records(
    records: &[TrackingRecord],
    migrator: &Migrator,
    schema: &LegacySchema,
) -> Result<(), String> {
    for record in records {
        let migration = migrator
            .migrations
            .iter()
            .find(|migration| migration.version == record.version)
            .ok_or_else(|| {
                format!(
                    "Bootstrap: database contains unknown migration v{}",
                    record.version
                )
            })?;
        if !record.success {
            return Err(format!(
                "Bootstrap: migration v{} is marked dirty",
                record.version
            ));
        }
        if record.checksum.as_slice() != migration.checksum.as_ref() {
            return Err(format!(
                "Bootstrap: migration v{} checksum does not match this build",
                record.version
            ));
        }
        if !schema.tracked_migration_is_consistent(record.version) {
            return Err(format!(
                "Bootstrap: migration v{} is tracked but its schema invariant is missing",
                record.version
            ));
        }
    }
    Ok(())
}

async fn seed_proven_migrations(
    transaction: &mut Transaction<'_, Sqlite>,
    migrator: &Migrator,
    schema: &LegacySchema,
    records: &[TrackingRecord],
) -> Result<(), String> {
    let recorded = records
        .iter()
        .map(|record| record.version)
        .collect::<HashSet<_>>();
    for migration in migrator.migrations.iter() {
        if recorded.contains(&migration.version) || !schema.proves_migration(migration.version) {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO _sqlx_migrations
             (version, description, installed_on, success, checksum, execution_time)
             VALUES (?, ?, datetime('now'), 1, ?, 0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut **transaction)
        .await
        .map_err(|error| {
            format!(
                "Bootstrap: failed to seed migration v{}: {error}",
                migration.version
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "migration_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "migration_runner_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "legacy_fixture_migration_test.rs"]
mod legacy_fixture_test;
