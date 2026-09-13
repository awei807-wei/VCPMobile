use super::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Sqlite, Transaction};

impl HashAggregator {
    pub async fn bubble_agent_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_agent_root_hash(tx, agent_id).await?;
        let updated = sqlx::query(
            "UPDATE agents SET content_hash = ? WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Agent {agent_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_group_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let root_hash = Self::compute_group_root_hash(tx, group_id).await?;
        let updated = sqlx::query(
            "UPDATE groups SET content_hash = ? WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(root_hash)
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Group {group_id} disappeared during hash update"));
        }
        Ok(())
    }

    pub async fn bubble_topic_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        let key = Self::resolve_topic_key(tx, topic_id).await?;
        Self::bubble_topic_hash_for_key(tx, &key).await
    }

    pub async fn bubble_from_topic_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<(), String> {
        Self::bubble_topic_hash_for_key(tx, key).await?;
        Self::bubble_owner_from_topic_key(tx, key).await
    }

    /// Refresh only the owning Agent/Group aggregate after a Topic config
    /// version changes. Message-only paths must keep using
    /// [`Self::bubble_from_topic_for_key`] so Topic content is refreshed too.
    pub async fn bubble_owner_from_topic_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<(), String> {
        match key.owner_type.as_str() {
            "agent" => Self::bubble_agent_hash(tx, &key.owner_id).await,
            "group" => Self::bubble_group_hash(tx, &key.owner_id).await,
            other => Err(format!(
                "Topic {} has unsupported owner type {other}",
                key.topic_id
            )),
        }
    }

    pub async fn bubble_from_topic(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        let key = Self::resolve_topic_key(tx, topic_id).await?;
        Self::bubble_from_topic_for_key(tx, &key).await
    }
}
