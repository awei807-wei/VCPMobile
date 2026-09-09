use super::{HashAggregator, HashInitializer};
use crate::vcp_modules::sync_types::compute_merkle_root;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};

impl HashAggregator {
    /// Compute a message root under a complete composite topic identity.
    pub async fn compute_topic_root_hash_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<String, String> {
        let rows = sqlx::query(
            "SELECT msg_id, content_hash FROM messages
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut leaves = Vec::with_capacity(rows.len());
        for row in rows {
            let message_id: String = row.try_get("msg_id").map_err(|error| {
                format!("Topic {} message id decode failed: {error}", key.topic_id)
            })?;
            let message_hash: String = row.try_get("content_hash").map_err(|error| {
                format!("Topic {} message hash decode failed: {error}", key.topic_id)
            })?;
            leaves.push(Self::compute_message_leaf_hash(&message_id, &message_hash));
        }
        Ok(compute_merkle_root(leaves))
    }

    /// Resolve the historical topic-only API to a unique composite identity.
    /// If old data contains colliding topic ids, fail closed rather than
    /// choosing an arbitrary owner's messages.
    pub(super) async fn resolve_topic_key(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<TopicKey, String> {
        let rows = sqlx::query(
            "SELECT owner_type, owner_id FROM topics
             WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if rows.len() != 1 {
            return Err(match rows.len() {
                0 => format!("Topic {topic_id} is missing or deleted"),
                count => format!("Topic {topic_id} is ambiguous across {count} owners"),
            });
        }
        let owner_type: String = rows[0]
            .try_get("owner_type")
            .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
        let owner_id: String = rows[0]
            .try_get("owner_id")
            .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
        let key = TopicKey::new(owner_type, owner_id, topic_id);
        if !key.is_valid() {
            return Err(format!("Topic {topic_id} has invalid owner identity"));
        }
        Ok(key)
    }

    /// Compatibility wrapper. Composite-aware callers should use the
    /// `_for_key` variant.
    pub async fn compute_topic_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<String, String> {
        let key = Self::resolve_topic_key(tx, topic_id).await?;
        Self::compute_topic_root_hash_for_key(tx, &key).await
    }

    pub async fn compute_agent_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<String, String> {
        Self::compute_owner_root_hash(tx, "agent", agent_id).await
    }

    pub async fn compute_group_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<String, String> {
        Self::compute_owner_root_hash(tx, "group", group_id).await
    }

    async fn compute_owner_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        owner_type: &str,
        owner_id: &str,
    ) -> Result<String, String> {
        let rows = sqlx::query(
            "SELECT topic_id, config_hash, content_hash FROM topics
             WHERE owner_type = ? AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(owner_type)
        .bind(owner_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut leaves = Vec::with_capacity(rows.len());
        for row in rows {
            let topic_id: String = row.try_get("topic_id").map_err(|error| {
                format!("{owner_type} {owner_id} topic id decode failed: {error}")
            })?;
            let config_hash: String = row.try_get("config_hash").map_err(|error| {
                format!("{owner_type} {owner_id} topic config hash decode failed: {error}")
            })?;
            let content_hash: String = row.try_get("content_hash").map_err(|error| {
                format!("{owner_type} {owner_id} topic content hash decode failed: {error}")
            })?;
            leaves.push(Self::compute_topic_leaf_hash(
                &topic_id,
                &config_hash,
                &content_hash,
            ));
        }
        Ok(compute_merkle_root(leaves))
    }

    pub async fn bubble_topic_hash_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<(), String> {
        let root_hash = Self::compute_topic_root_hash_for_key(tx, key).await?;
        let config_hash = match key.owner_type.as_str() {
            "agent" => {
                let dto = HashInitializer::load_agent_topic_dto_for_key(tx, key).await?;
                Self::compute_agent_topic_metadata_hash(&dto)
            }
            "group" => {
                let dto = HashInitializer::load_group_topic_dto_for_key(tx, key).await?;
                Self::compute_group_topic_metadata_hash(&dto)
            }
            _ => {
                return Err(format!(
                    "Topic {} has unsupported owner type {}",
                    key.topic_id, key.owner_type
                ));
            }
        };

        let updated = sqlx::query(
            "UPDATE topics SET content_hash = ?, config_hash = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(config_hash)
        .bind(&key.owner_type)
        .bind(&key.owner_id)
        .bind(&key.topic_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Topic {}/{} disappeared during hash update",
                key.owner_id, key.topic_id
            ));
        }
        Ok(())
    }
}
