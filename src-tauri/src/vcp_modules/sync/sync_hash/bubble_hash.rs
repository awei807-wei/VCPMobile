use super::HashAggregator;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
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

    pub async fn bubble_topic_hash_with_meta_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
        title: &str,
        created_at: i64,
        locked: bool,
        unread: bool,
    ) -> Result<(), String> {
        let root_hash = Self::compute_topic_root_hash_for_key(tx, key).await?;
        let config_hash = match key.owner_type.as_str() {
            "agent" => Self::compute_agent_topic_metadata_hash(&AgentTopicSyncDTO {
                id: key.topic_id.clone(),
                name: title.to_string(),
                created_at,
                locked,
                unread,
                owner_id: key.owner_id.clone(),
            }),
            "group" => Self::compute_group_topic_metadata_hash(&GroupTopicSyncDTO {
                id: key.topic_id.clone(),
                name: title.to_string(),
                created_at,
                owner_id: key.owner_id.clone(),
            }),
            _ => {
                return Err(format!(
                    "Topic {} has unsupported owner type {}",
                    key.topic_id, key.owner_type
                ));
            }
        };

        Self::update_topic_hashes(tx, key, root_hash, config_hash).await
    }

    pub async fn bubble_topic_hash_with_meta(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
        owner_type: &str,
        title: &str,
        created_at: i64,
        locked: bool,
        unread: bool,
    ) -> Result<(), String> {
        let key = Self::resolve_topic_key(tx, topic_id).await?;
        if key.owner_type != owner_type {
            return Err(format!(
                "Topic {topic_id} owner type mismatch: expected {owner_type}, got {}",
                key.owner_type
            ));
        }
        Self::bubble_topic_hash_with_meta_for_key(tx, &key, title, created_at, locked, unread).await
    }

    pub async fn bubble_from_topic_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<(), String> {
        Self::bubble_topic_hash_for_key(tx, key).await?;
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

    async fn update_topic_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
        root_hash: String,
        config_hash: String,
    ) -> Result<(), String> {
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
