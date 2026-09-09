//! Debug-only database fixtures for exercising the Wire 1.4 push boundary.
//!
//! This module is intentionally compiled only for debug builds.  The command
//! is not a general SQL escape hatch: it accepts only deterministic
//! `wire14-e2e-*` fixture namespaces and only reads scale hashes or creates
//! the malformed attachment relation needed by the Android E2E contract.

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::Transaction;
use std::borrow::Cow;
use tauri::{AppHandle, State};

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wire14DebugTopicHash {
    pub id: String,
    pub config_hash: String,
    pub content_hash: String,
}

#[path = "wire14_debug/support.rs"]
mod support;

use support::{
    ensure_live_owner, ensure_live_topic, ensure_relation_is_new,
    insert_invalid_attachment_catalog, load_attachment_hashes, load_live_message,
    load_scale_topic_hashes, next_attachment_order, next_update_clock, validate_scale_owner,
    validate_target,
};

struct InvalidAttachmentUpdate {
    next_order: i32,
    updated_at: i64,
    content_hash: String,
}

/// Read canonical hashes for the synthetic scale owner used by Android E2E.
///
/// This command deliberately exposes no general topic query: both the owner
/// type and the owner id namespace are fixed to the debug scale fixture.
#[tauri::command]
pub async fn debug_get_wire14_scale_topic_hashes(
    db_state: State<'_, DbState>,
    owner_type: String,
    owner_id: String,
) -> Result<Vec<Wire14DebugTopicHash>, String> {
    validate_scale_owner(&owner_type, &owner_id)?;
    load_scale_topic_hashes(&db_state.pool, &owner_type, &owner_id).await
}

/// Inject one malformed attachment relation into a pre-existing E2E message.
///
/// The normal message append path rejects malformed hashes before persistence.
/// This narrowly scoped fixture command deliberately bypasses that validation
/// so the next real sync attempt can serialize the relation and exercise the
/// desktop plugin's `MOBILE_ATTACHMENT_INVALID` response.  All identity and
/// hash checks remain fail-closed, and this command is absent from release
/// builds because the whole module is guarded by `debug_assertions`.
#[tauri::command]
pub async fn debug_inject_invalid_attachment_for_wire14(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    owner_type: String,
    owner_id: String,
    topic_id: String,
    msg_id: String,
    attachment_hash: String,
) -> Result<(), String> {
    let _gate = crate::vcp_modules::file_manager::attachment_gc_gate()
        .read()
        .await;
    let roots =
        crate::vcp_modules::infra::maintenance_manager::managed_attachment_roots(&app_handle)?;
    validate_target(&owner_type, &owner_id, &topic_id, &msg_id, &attachment_hash)?;

    let connection = db_state
        .pool
        .acquire()
        .await
        .map_err(|error| format!("Acquire debug fixture connection failed: {error}"))?;
    let mut tx = Transaction::begin(connection, Some(Cow::Borrowed("BEGIN IMMEDIATE")))
        .await
        .map_err(|error| format!("Begin debug fixture transaction failed: {error}"))?;

    ensure_live_owner(&mut tx, &owner_type, &owner_id).await?;
    let key = TopicKey::new(&owner_type, &owner_id, &topic_id);
    ensure_live_topic(&mut tx, &key).await?;
    let update = prepare_invalid_attachment(&mut tx, &key, &msg_id, &attachment_hash).await?;
    insert_attachment_relation(&mut tx, &key, &msg_id, &attachment_hash, &update).await?;
    crate::vcp_modules::infra::maintenance_manager::clear_live_attachment_unlink_debts(
        &mut tx,
        &attachment_hash,
        &roots,
    )
    .await?;
    update_message_hash(&mut tx, &key, &msg_id, &update).await?;
    update_render_hash(&mut tx, &key, &msg_id, &update).await?;
    update_topic_clock(&mut tx, &key, &update).await?;

    HashAggregator::bubble_from_topic_for_key(&mut tx, &key).await?;
    tx.commit()
        .await
        .map_err(|error| format!("Commit debug attachment fixture failed: {error}"))
}

async fn prepare_invalid_attachment(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    attachment_hash: &str,
) -> Result<InvalidAttachmentUpdate, String> {
    let message = load_live_message(tx, key, msg_id).await?;
    ensure_relation_is_new(tx, key, msg_id, attachment_hash).await?;
    let next_order = next_attachment_order(tx, key, msg_id).await?;
    let updated_at = next_update_clock(message.updated_at)?;
    let mut attachment_hashes = load_attachment_hashes(tx, key, msg_id).await?;
    attachment_hashes.push(attachment_hash.to_string());
    let content_hash = HashAggregator::compute_message_fingerprint_with_identity(
        msg_id,
        &message.role,
        message.name.as_deref(),
        &message.content,
        message.timestamp,
        message.agent_id.as_deref(),
        &attachment_hashes,
    );
    insert_invalid_attachment_catalog(tx, attachment_hash, updated_at).await?;
    Ok(InvalidAttachmentUpdate {
        next_order,
        updated_at,
        content_hash,
    })
}

async fn insert_attachment_relation(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    attachment_hash: &str,
    update: &InvalidAttachmentUpdate,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO message_attachments (
            owner_type, owner_id, topic_id, msg_id, hash, attachment_order,
            display_name, src, status, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .bind(attachment_hash)
    .bind(update.next_order)
    .bind("wire14-e2e-invalid.bin")
    .bind("")
    .bind("invalid")
    .bind(update.updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Insert debug attachment relation failed: {error}"))?;
    Ok(())
}

async fn update_message_hash(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    update: &InvalidAttachmentUpdate,
) -> Result<(), String> {
    let updated = sqlx::query(
        "UPDATE messages SET content_hash = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?
           AND deleted_at IS NULL",
    )
    .bind(&update.content_hash)
    .bind(update.updated_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Update debug message hash failed: {error}"))?;
    if updated.rows_affected() != 1 {
        return Err("Debug fixture message disappeared during hash update".to_string());
    }
    Ok(())
}

async fn update_render_hash(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    msg_id: &str,
    update: &InvalidAttachmentUpdate,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE render_cache SET content_hash = ?, updated_at = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&update.content_hash)
    .bind(update.updated_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .bind(msg_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Update debug render hash failed: {error}"))?;
    Ok(())
}

async fn update_topic_clock(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &TopicKey,
    update: &InvalidAttachmentUpdate,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE topics
         SET updated_at = MAX(updated_at, ?),
             last_message_updated_at = MAX(last_message_updated_at, ?)
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
           AND deleted_at IS NULL",
    )
    .bind(update.updated_at)
    .bind(update.updated_at)
    .bind(&key.owner_type)
    .bind(&key.owner_id)
    .bind(&key.topic_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Update debug topic clock failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scale_hash_query_is_composite_and_stably_sorted() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE topics (
                owner_type TEXT, owner_id TEXT, topic_id TEXT,
                config_hash TEXT, content_hash TEXT, deleted_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (owner_type, owner_id, topic_id, deleted_at) in [
            ("agent", "wire14-e2e-scale-owner-run", "topic-b", None),
            ("agent", "wire14-e2e-scale-owner-run", "topic-a", None),
            ("group", "wire14-e2e-scale-owner-run", "topic-z", None),
            ("agent", "wire14-e2e-scale-owner-other", "topic-x", None),
            (
                "agent",
                "wire14-e2e-scale-owner-run",
                "topic-deleted",
                Some(1),
            ),
        ] {
            sqlx::query("INSERT INTO topics VALUES (?, ?, ?, ?, ?, ?)")
                .bind(owner_type)
                .bind(owner_id)
                .bind(topic_id)
                .bind(format!("config-{topic_id}"))
                .bind(format!("content-{topic_id}"))
                .bind(deleted_at)
                .execute(&pool)
                .await
                .unwrap();
        }
        let rows = support::load_scale_topic_hashes(&pool, "agent", "wire14-e2e-scale-owner-run")
            .await
            .unwrap();
        assert_eq!(
            rows,
            vec![
                Wire14DebugTopicHash {
                    id: "topic-a".to_string(),
                    config_hash: "config-topic-a".to_string(),
                    content_hash: "content-topic-a".to_string(),
                },
                Wire14DebugTopicHash {
                    id: "topic-b".to_string(),
                    config_hash: "config-topic-b".to_string(),
                    content_hash: "content-topic-b".to_string(),
                },
            ]
        );
    }
}
