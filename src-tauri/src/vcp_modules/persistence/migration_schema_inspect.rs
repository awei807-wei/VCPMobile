use sqlx::{Row, Sqlite, Transaction};

use super::LegacySchema;

pub(crate) async fn inspect_legacy_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<LegacySchema, String> {
    let mut schema = inspect_core_schema(transaction).await?;
    schema.merge(inspect_composite_schema(transaction).await?);
    schema.merge(inspect_extension_schema(transaction).await?);
    Ok(schema)
}

async fn inspect_core_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<LegacySchema, String> {
    Ok(LegacySchema {
        attachment_deleted_at: column_exists(transaction, "message_attachments", "deleted_at")
            .await?,
        messages_fts: table_exists(transaction, "messages_fts").await?,
        fts_topic_identity: fts_delete_triggers_include_topic_identity(transaction).await?,
        active_generations: table_exists(transaction, "active_generations").await?,
        render_content_hash: column_exists(transaction, "render_cache", "content_hash").await?,
        render_schema_version: column_exists(
            transaction,
            "render_cache",
            "renderer_schema_version",
        )
        .await?,
        avatar_deleted_at: column_exists(transaction, "avatars", "deleted_at").await?,
        ..LegacySchema::default()
    })
}

async fn inspect_composite_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<LegacySchema, String> {
    Ok(LegacySchema {
        composite_topics: primary_key_matches(
            transaction,
            "topics",
            &["owner_type", "owner_id", "topic_id"],
        )
        .await?,
        composite_messages: primary_key_matches(
            transaction,
            "messages",
            &["owner_type", "owner_id", "topic_id", "msg_id"],
        )
        .await?,
        composite_render_cache: primary_key_matches(
            transaction,
            "render_cache",
            &["owner_type", "owner_id", "topic_id", "msg_id"],
        )
        .await?,
        composite_message_attachments: primary_key_matches(
            transaction,
            "message_attachments",
            &[
                "owner_type",
                "owner_id",
                "topic_id",
                "msg_id",
                "attachment_order",
            ],
        )
        .await?,
        composite_active_generations: primary_key_matches(
            transaction,
            "active_generations",
            &["owner_type", "owner_id", "topic_id", "msg_id"],
        )
        .await?,
        topic_activity_clock: column_exists(transaction, "topics", "last_message_updated_at")
            .await?,
        fts_composite_identity: column_exists(transaction, "messages_fts", "owner_type").await?
            && column_exists(transaction, "messages_fts", "owner_id").await?,
        ..LegacySchema::default()
    })
}

async fn inspect_extension_schema(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<LegacySchema, String> {
    let tables = inspect_extension_tables(transaction).await?;
    inspect_extension_schema_details(transaction, tables).await
}

#[derive(Debug, Clone, Copy)]
struct ExtensionTables {
    group_member_tags: bool,
    recovery_cleanup_outbox: bool,
    attachment_gc_unlink_outbox: bool,
    attachment_gc_unlink_live_references: bool,
    message_unread_receipts: bool,
}

async fn inspect_extension_tables(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<ExtensionTables, String> {
    let group_member_tags_table = table_exists(transaction, "group_member_tags").await?;
    let recovery_cleanup_outbox_table =
        table_exists(transaction, "recovery_cleanup_outbox").await?;
    let attachment_gc_unlink_outbox_table =
        table_exists(transaction, "attachment_gc_unlink_outbox").await?;
    let attachment_gc_unlink_live_references_table =
        table_exists(transaction, "attachment_gc_unlink_live_references").await?;
    let message_unread_receipts_table =
        table_exists(transaction, "message_unread_receipts").await?;
    Ok(ExtensionTables {
        group_member_tags: group_member_tags_table,
        recovery_cleanup_outbox: recovery_cleanup_outbox_table,
        attachment_gc_unlink_outbox: attachment_gc_unlink_outbox_table,
        attachment_gc_unlink_live_references: attachment_gc_unlink_live_references_table,
        message_unread_receipts: message_unread_receipts_table,
    })
}

async fn inspect_extension_schema_details(
    transaction: &mut Transaction<'_, Sqlite>,
    tables: ExtensionTables,
) -> Result<LegacySchema, String> {
    Ok(LegacySchema {
        group_member_tags_table: tables.group_member_tags,
        group_member_tags: tables.group_member_tags
            && group_member_tags_schema_matches(transaction).await?,
        helper_generation: column_exists(transaction, "active_generations", "helper_generation")
            .await?,
        recovery_cleanup_outbox_table: tables.recovery_cleanup_outbox,
        recovery_cleanup_outbox: tables.recovery_cleanup_outbox
            && recovery_cleanup_outbox_schema_matches(transaction).await?,
        attachment_gc_unlink_outbox_table: tables.attachment_gc_unlink_outbox,
        attachment_gc_unlink_outbox: tables.attachment_gc_unlink_outbox
            && attachment_gc_unlink_outbox_schema_matches(transaction).await?,
        attachment_gc_unlink_live_references_table: tables.attachment_gc_unlink_live_references,
        attachment_gc_unlink_live_references: tables.attachment_gc_unlink_live_references
            && attachment_gc_unlink_live_references_schema_matches(transaction).await?,
        attachment_gc_internal_path_index: attachment_gc_reference_index_matches(
            transaction,
            "idx_attachments_internal_path",
            "internal_path",
            "BINARY",
        )
        .await?,
        attachment_gc_thumbnail_path_index: attachment_gc_reference_index_matches(
            transaction,
            "idx_attachments_thumbnail_path",
            "thumbnail_path",
            "BINARY",
        )
        .await?,
        attachment_gc_hash_index: attachment_gc_reference_index_matches(
            transaction,
            "idx_attachments_hash_nocase",
            "hash",
            "NOCASE",
        )
        .await?,
        attachment_gc_message_hash_index: attachment_gc_reference_index_matches(
            transaction,
            "idx_message_attachments_hash_nocase",
            "hash",
            "NOCASE",
        )
        .await?,
        message_unread_receipts_table: tables.message_unread_receipts,
        message_unread_receipts: tables.message_unread_receipts
            && message_unread_receipts_schema_matches(transaction).await?,
        ..LegacySchema::default()
    })
}

async fn message_unread_receipts_schema_matches(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let expected = [
        ("owner_type", "TEXT", 1_i64, 1_i64),
        ("owner_id", "TEXT", 1, 2),
        ("topic_id", "TEXT", 1, 3),
        ("msg_id", "TEXT", 1, 4),
        ("created_at", "BIGINT", 1, 0),
        ("counted_unread", "INTEGER", 1, 0),
    ];
    Ok(
        table_columns_match(transaction, "message_unread_receipts", &expected).await?
            && index_exists(transaction, "idx_message_unread_receipts_topic").await?,
    )
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

async fn primary_key_matches(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &str,
    expected: &[&str],
) -> Result<bool, String> {
    let actual = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info(?) WHERE pk > 0 ORDER BY pk",
    )
    .bind(table)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect primary key for {table}: {error}"))?;
    Ok(actual
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied()))
}

async fn group_member_tags_schema_matches(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let expected = [
        ("group_id", "TEXT", 1_i64, 1_i64),
        ("agent_id", "TEXT", 1, 2),
        ("member_tag", "TEXT", 1, 0),
        ("updated_at", "BIGINT", 1, 0),
    ];
    Ok(
        table_columns_match(transaction, "group_member_tags", &expected).await?
            && index_exists(transaction, "idx_group_member_tags_group").await?,
    )
}

async fn recovery_cleanup_outbox_schema_matches(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let expected = [
        ("claimed_path", "TEXT", 1_i64, 1_i64),
        ("owner_type", "TEXT", 1, 0),
        ("owner_id", "TEXT", 1, 0),
        ("topic_id", "TEXT", 1, 0),
        ("msg_id", "TEXT", 1, 0),
        ("helper_generation", "BIGINT", 1, 0),
        ("created_at", "BIGINT", 1, 0),
    ];
    Ok(
        table_columns_match(transaction, "recovery_cleanup_outbox", &expected).await?
            && index_exists(transaction, "idx_recovery_cleanup_outbox_created").await?,
    )
}

async fn attachment_gc_unlink_outbox_schema_matches(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let expected = [
        ("root_kind", "TEXT", 1_i64, 1_i64),
        ("relative_path", "TEXT", 1, 2),
        ("created_at", "BIGINT", 1, 0),
    ];
    Ok(
        table_columns_match(transaction, "attachment_gc_unlink_outbox", &expected).await?
            && index_exists(transaction, "idx_attachment_gc_unlink_outbox_created").await?,
    )
}

async fn attachment_gc_unlink_live_references_schema_matches(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let expected = [
        ("root_kind", "TEXT", 1_i64, 1_i64),
        ("relative_path", "TEXT", 1, 2),
        ("hash", "TEXT", 1, 3),
    ];
    Ok(table_columns_match(
        transaction,
        "attachment_gc_unlink_live_references",
        &expected,
    )
    .await?
        && index_exists(transaction, "idx_attachment_gc_unlink_live_references_hash").await?)
}

async fn table_columns_match(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &str,
    expected: &[(&str, &str, i64, i64)],
) -> Result<bool, String> {
    let rows =
        sqlx::query("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?) ORDER BY cid")
            .bind(table)
            .fetch_all(&mut **transaction)
            .await
            .map_err(|error| {
                format!("Bootstrap: failed to inspect columns for {table}: {error}")
            })?;
    if rows.len() != expected.len() {
        return Ok(false);
    }
    rows.iter()
        .zip(expected.iter())
        .map(|(row, (name, sql_type, not_null, primary_key))| {
            let actual_name = row.try_get::<String, _>("name")?;
            let actual_type = row.try_get::<String, _>("type")?;
            let actual_not_null = row.try_get::<i64, _>("notnull")?;
            let actual_primary_key = row.try_get::<i64, _>("pk")?;
            Ok(actual_name == *name
                && actual_type.eq_ignore_ascii_case(sql_type)
                && actual_not_null == *not_null
                && actual_primary_key == *primary_key)
        })
        .collect::<Result<Vec<bool>, sqlx::Error>>()
        .map(|matches| matches.into_iter().all(|matched| matched))
        .map_err(|error| format!("Bootstrap: failed to decode columns for {table}: {error}"))
}

async fn index_exists(
    transaction: &mut Transaction<'_, Sqlite>,
    index: &str,
) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?)",
    )
    .bind(index)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect index {index}: {error}"))
}

async fn attachment_gc_reference_index_matches(
    transaction: &mut Transaction<'_, Sqlite>,
    index: &str,
    expected_column: &str,
    expected_collation: &str,
) -> Result<bool, String> {
    let rows = sqlx::query(
        "SELECT name, coll
         FROM pragma_index_xinfo(?)
         WHERE key = 1
         ORDER BY seqno",
    )
    .bind(index)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect index {index}: {error}"))?;
    if rows.len() != 1 {
        return Ok(false);
    }
    let name = rows[0]
        .try_get::<Option<String>, _>("name")
        .map_err(|error| format!("Bootstrap: failed to decode index {index}: {error}"))?;
    let collation = rows[0]
        .try_get::<Option<String>, _>("coll")
        .map_err(|error| format!("Bootstrap: failed to decode index {index}: {error}"))?;
    Ok(name.as_deref() == Some(expected_column)
        && collation
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(expected_collation)))
}

async fn fts_delete_triggers_include_topic_identity(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool, String> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'trigger'
           AND name IN ('after_messages_physical_delete', 'after_messages_logical_delete')
           AND instr(lower(sql), 'topic_id') > 0",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| format!("Bootstrap: failed to inspect FTS delete triggers: {error}"))?;
    Ok(count == 2)
}
