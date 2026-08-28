use std::collections::HashSet;

use sqlx::{migrate::Migrator, Pool, Row, Sqlite, Transaction};

const MIGRATION_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version        BIGINT PRIMARY KEY,
    description    TEXT NOT NULL,
    installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success        BOOLEAN NOT NULL,
    checksum       BLOB NOT NULL,
    execution_time BIGINT NOT NULL
)";

pub(super) async fn run_migrations(pool: &Pool<Sqlite>) -> Result<(), String> {
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
struct LegacySchema {
    attachment_deleted_at: bool,
    messages_fts: bool,
    active_generations: bool,
    render_content_hash: bool,
    render_schema_version: bool,
    avatar_deleted_at: bool,
}

impl LegacySchema {
    fn validate_partial_states(&self) -> Result<(), String> {
        if self.render_content_hash != self.render_schema_version {
            return Err(
                "Bootstrap: render_cache identity migration is only partially applied; database left unchanged"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn proves_migration(&self, version: i64) -> bool {
        match version {
            1 => true,
            2 => self.attachment_deleted_at,
            3 => self.messages_fts,
            5 => self.active_generations,
            6 => self.render_content_hash && self.render_schema_version,
            7 => self.avatar_deleted_at,
            _ => false,
        }
    }

    fn tracked_migration_is_consistent(&self, version: i64) -> bool {
        match version {
            1 | 2 | 3 | 5 | 6 | 7 => self.proves_migration(version),
            _ => true,
        }
    }
}

async fn inspect_legacy_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<LegacySchema, String> {
    Ok(LegacySchema {
        attachment_deleted_at: column_exists(transaction, "message_attachments", "deleted_at")
            .await?,
        messages_fts: table_exists(transaction, "messages_fts").await?,
        active_generations: table_exists(transaction, "active_generations").await?,
        render_content_hash: column_exists(transaction, "render_cache", "content_hash").await?,
        render_schema_version: column_exists(
            transaction,
            "render_cache",
            "renderer_schema_version",
        )
        .await?,
        avatar_deleted_at: column_exists(transaction, "avatars", "deleted_at").await?,
    })
}

async fn table_exists(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
    )
    .bind(table)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect table {table}: {error}"))
}

async fn column_exists(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &str,
    column: &str,
) -> Result<bool, String> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?) WHERE name = ?)")
        .bind(table)
        .bind(column)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|error| format!("Bootstrap: failed to inspect column {table}.{column}: {error}"))
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
