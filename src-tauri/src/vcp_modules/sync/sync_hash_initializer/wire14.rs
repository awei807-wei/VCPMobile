use super::HashInitializer;
use crate::vcp_modules::persistence::message_content_storage::decode_message_content;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::sync_types::is_sha256;
use crate::vcp_modules::topic_types::{MessageKey, OwnerKey, TopicKey};
use sqlx::{Row, Sqlite, Transaction};
use std::borrow::Cow;
use std::collections::HashMap;

const HASH_CONTRACT_SETTING: &str = "sync_wire_hash_contract";
const HASH_CONTRACT_VERSION: &str = "wire-1.4-message-v1";
const MESSAGE_REHASH_PAGE_SIZE: i64 = 256;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Wire14HashInitStats {
    pub(crate) full_rebuild: bool,
    pub(crate) messages: usize,
    pub(crate) topics: usize,
    pub(crate) agents: usize,
    pub(crate) groups: usize,
}

impl HashInitializer {
    /// Upgrade retained Wire 1.2 hashes to the current Wire 1.4 canonical
    /// contract before a network session can expose local manifest state.
    pub(crate) async fn ensure_wire14_hashes(
        pool: &sqlx::SqlitePool,
    ) -> Result<Wire14HashInitStats, String> {
        let mut tx = begin_immediate(pool).await?;
        let marker: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(HASH_CONTRACT_SETTING)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| format!("Read sync hash contract marker failed: {error}"))?;

        let mut stats = Wire14HashInitStats {
            full_rebuild: marker.as_deref() != Some(HASH_CONTRACT_VERSION),
            ..Wire14HashInitStats::default()
        };
        if stats.full_rebuild {
            stats.messages = rehash_all_messages(&mut tx).await?;
            stats.agents = recompute_agent_configs(&mut tx, true).await?;
            stats.groups = recompute_group_configs(&mut tx, true).await?;
            stats.topics = recompute_topics(&mut tx, true).await?;
            write_contract_marker(&mut tx).await?;
        } else {
            stats.agents = recompute_agent_configs(&mut tx, false).await?;
            stats.groups = recompute_group_configs(&mut tx, false).await?;
            stats.topics = recompute_topics(&mut tx, false).await?;
        }
        recompute_owner_roots(&mut tx).await?;
        tx.commit()
            .await
            .map_err(|error| format!("Commit sync hash initialization failed: {error}"))?;
        log::info!(
            "[HashInitializer] Wire 1.4 hashes ready: full_rebuild={}, messages={}, topics={}, agents={}, groups={}",
            stats.full_rebuild,
            stats.messages,
            stats.topics,
            stats.agents,
            stats.groups
        );
        Ok(stats)
    }
}

async fn begin_immediate(pool: &sqlx::SqlitePool) -> Result<Transaction<'static, Sqlite>, String> {
    let connection = pool
        .acquire()
        .await
        .map_err(|error| format!("Acquire sync hash connection failed: {error}"))?;
    Transaction::begin(connection, Some(Cow::Borrowed("BEGIN IMMEDIATE")))
        .await
        .map_err(|error| format!("Begin sync hash write transaction failed: {error}"))
}

async fn rehash_all_messages(tx: &mut Transaction<'_, Sqlite>) -> Result<usize, String> {
    let mut last_rowid = 0_i64;
    let mut count = 0_usize;
    loop {
        let rows = sqlx::query(
            "SELECT rowid, owner_type, owner_id, topic_id, msg_id, role, name,
                    agent_id, content, timestamp
             FROM messages
             WHERE deleted_at IS NULL AND rowid > ?
             ORDER BY rowid LIMIT ?",
        )
        .bind(last_rowid)
        .bind(MESSAGE_REHASH_PAGE_SIZE)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Load messages for Wire 1.4 rehash failed: {error}"))?;
        if rows.is_empty() {
            break;
        }
        let upper_rowid = rows
            .last()
            .and_then(|row| row.try_get::<i64, _>("rowid").ok())
            .ok_or_else(|| "Decode message rehash cursor failed".to_string())?;
        let attachments = load_attachment_hashes(tx, last_rowid, upper_rowid).await?;
        for row in rows {
            rehash_message_row(tx, &row, &attachments).await?;
            count += 1;
        }
        last_rowid = upper_rowid;
    }
    Ok(count)
}

async fn load_attachment_hashes(
    tx: &mut Transaction<'_, Sqlite>,
    lower_rowid: i64,
    upper_rowid: i64,
) -> Result<HashMap<MessageKey, Vec<String>>, String> {
    let rows = sqlx::query(
        "SELECT m.owner_type, m.owner_id, m.topic_id, m.msg_id, ma.hash
         FROM messages m
         JOIN message_attachments ma
           ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id
          AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id
         WHERE m.deleted_at IS NULL AND ma.deleted_at IS NULL
           AND m.rowid > ? AND m.rowid <= ?
         ORDER BY ma.attachment_order, ma.hash",
    )
    .bind(lower_rowid)
    .bind(upper_rowid)
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Load attachment hashes for message rehash failed: {error}"))?;
    let mut result = HashMap::new();
    for row in rows {
        let key = decode_message_key(&row)?;
        let hash: String = row
            .try_get("hash")
            .map_err(|error| format!("Decode attachment hash failed: {error}"))?;
        let canonical_hash = hash.to_ascii_lowercase();
        if !is_sha256(&canonical_hash, false) {
            return Err(format!(
                "Message {}/{}/{} contains an invalid attachment hash",
                key.topic.owner_id, key.topic.topic_id, key.msg_id
            ));
        }
        // Legacy CAS catalogs may use upper-case hexadecimal names. Keep the
        // relation and catalog keys untouched so a case-sensitive SQLite join
        // and the physical CAS path remain valid; only the Wire 1.4 canonical
        // fingerprint input is normalized. Duplicate references are retained
        // because attachment_order, rather than the hash, gives a relation
        // its identity.
        result
            .entry(key)
            .or_insert_with(Vec::new)
            .push(canonical_hash);
    }
    Ok(result)
}

async fn rehash_message_row(
    tx: &mut Transaction<'_, Sqlite>,
    row: &sqlx::sqlite::SqliteRow,
    attachments: &HashMap<MessageKey, Vec<String>>,
) -> Result<(), String> {
    let rowid: i64 = row
        .try_get("rowid")
        .map_err(|error| format!("Decode message rehash rowid failed: {error}"))?;
    let key = decode_message_key(row)?;
    let role: String = row
        .try_get("role")
        .map_err(|error| format!("Decode message role failed: {error}"))?;
    if role.is_empty() {
        return Err(format!("Message {} has an empty role", key.msg_id));
    }
    let timestamp = u64::try_from(
        row.try_get::<i64, _>("timestamp")
            .map_err(|error| format!("Decode message timestamp failed: {error}"))?,
    )
    .map_err(|_| format!("Message {} has a negative timestamp", key.msg_id))?;
    let content = decode_message_content(row, "content")?;
    let name: Option<String> = row
        .try_get("name")
        .map_err(|error| format!("Decode message name failed: {error}"))?;
    let agent_id: Option<String> = row
        .try_get("agent_id")
        .map_err(|error| format!("Decode message agent id failed: {error}"))?;
    let hash = HashAggregator::compute_message_fingerprint_with_identity(
        &key.msg_id,
        &role,
        name.as_deref(),
        &content,
        timestamp,
        agent_id.as_deref(),
        attachments.get(&key).map(Vec::as_slice).unwrap_or_default(),
    );
    let updated =
        sqlx::query("UPDATE messages SET content_hash = ? WHERE rowid = ? AND deleted_at IS NULL")
            .bind(&hash)
            .bind(rowid)
            .execute(&mut **tx)
            .await
            .map_err(|error| format!("Update Wire 1.4 message hash failed: {error}"))?;
    if updated.rows_affected() != 1 {
        return Err(format!("Message {} disappeared during rehash", key.msg_id));
    }
    sqlx::query(
        "UPDATE render_cache SET content_hash = ?
         WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND msg_id = ?",
    )
    .bind(&hash)
    .bind(&key.topic.owner_type)
    .bind(&key.topic.owner_id)
    .bind(&key.topic.topic_id)
    .bind(&key.msg_id)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Update rehashed render cache identity failed: {error}"))?;
    Ok(())
}

fn decode_message_key(row: &sqlx::sqlite::SqliteRow) -> Result<MessageKey, String> {
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Decode message owner type failed: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Decode message owner id failed: {error}"))?;
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Decode message topic id failed: {error}"))?;
    let msg_id: String = row
        .try_get("msg_id")
        .map_err(|error| format!("Decode message id failed: {error}"))?;
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), msg_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err("Message rehash encountered an invalid composite identity".to_string())
    }
}

async fn recompute_topics(tx: &mut Transaction<'_, Sqlite>, all: bool) -> Result<usize, String> {
    let rows = sqlx::query(
        "SELECT owner_type, owner_id, topic_id, config_hash, content_hash
         FROM topics WHERE deleted_at IS NULL
         ORDER BY owner_type, owner_id, topic_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Load topics for hash initialization failed: {error}"))?;
    let mut count = 0;
    for row in rows {
        let key = TopicKey::new(
            row.try_get::<String, _>("owner_type")
                .map_err(|error| error.to_string())?,
            row.try_get::<String, _>("owner_id")
                .map_err(|error| error.to_string())?,
            row.try_get::<String, _>("topic_id")
                .map_err(|error| error.to_string())?,
        );
        let config_hash: String = row
            .try_get("config_hash")
            .map_err(|error| error.to_string())?;
        let content_hash: String = row
            .try_get("content_hash")
            .map_err(|error| error.to_string())?;
        if all || !is_sha256(&config_hash, false) || !is_sha256(&content_hash, true) {
            HashAggregator::bubble_topic_hash_for_key(tx, &key).await?;
            count += 1;
        }
    }
    Ok(count)
}

async fn recompute_agent_configs(
    tx: &mut Transaction<'_, Sqlite>,
    all: bool,
) -> Result<usize, String> {
    let rows = sqlx::query(
        "SELECT agent_id, config_hash FROM agents WHERE deleted_at IS NULL ORDER BY agent_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Load Agent hashes failed: {error}"))?;
    let mut count = 0;
    for row in rows {
        let id: String = row.try_get("agent_id").map_err(|error| error.to_string())?;
        let hash: String = row
            .try_get("config_hash")
            .map_err(|error| error.to_string())?;
        if all || !is_sha256(&hash, false) {
            HashInitializer::recompute_agent_config_hash(tx, &id).await?;
            count += 1;
        }
    }
    Ok(count)
}

async fn recompute_group_configs(
    tx: &mut Transaction<'_, Sqlite>,
    all: bool,
) -> Result<usize, String> {
    let rows = sqlx::query(
        "SELECT group_id, config_hash FROM groups WHERE deleted_at IS NULL ORDER BY group_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Load Group hashes failed: {error}"))?;
    let mut count = 0;
    for row in rows {
        let id: String = row.try_get("group_id").map_err(|error| error.to_string())?;
        let hash: String = row
            .try_get("config_hash")
            .map_err(|error| error.to_string())?;
        if all || !is_sha256(&hash, false) {
            HashInitializer::recompute_group_config_hash(tx, &id).await?;
            count += 1;
        }
    }
    Ok(count)
}

async fn recompute_owner_roots(tx: &mut Transaction<'_, Sqlite>) -> Result<(), String> {
    let rows = sqlx::query(
        "SELECT 'agent' AS owner_type, agent_id AS owner_id FROM agents WHERE deleted_at IS NULL
         UNION ALL
         SELECT 'group' AS owner_type, group_id AS owner_id FROM groups WHERE deleted_at IS NULL
         ORDER BY owner_type, owner_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(|error| format!("Load owner identities for hash initialization failed: {error}"))?;
    for row in rows {
        let key = OwnerKey::new(
            row.try_get::<String, _>("owner_type")
                .map_err(|error| error.to_string())?,
            row.try_get::<String, _>("owner_id")
                .map_err(|error| error.to_string())?,
        );
        match key.owner_type.as_str() {
            "agent" => HashAggregator::bubble_agent_hash(tx, &key.owner_id).await?,
            "group" => HashAggregator::bubble_group_hash(tx, &key.owner_id).await?,
            _ => return Err("Hash initialization loaded an invalid owner type".to_string()),
        }
    }
    Ok(())
}

async fn write_contract_marker(tx: &mut Transaction<'_, Sqlite>) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(HASH_CONTRACT_SETTING)
    .bind(HASH_CONTRACT_VERSION)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| format!("Write sync hash contract marker failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "wire14_tests.rs"]
mod tests;
