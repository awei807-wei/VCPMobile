use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::{compute_deterministic_hash, compute_merkle_root};

use sqlx::{Row, Sqlite, Transaction};

pub struct HashAggregator;

impl HashAggregator {
    pub fn compute_message_fingerprint(content: &str, attachment_hashes: &[String]) -> String {
        let mut sorted_hashes = attachment_hashes.to_vec();
        sorted_hashes.sort();

        let mut fingerprint_map = serde_json::Map::new();
        fingerprint_map.insert(
            "content".to_string(),
            serde_json::Value::String(content.to_string()),
        );
        if !sorted_hashes.is_empty() {
            fingerprint_map.insert(
                "attachmentHashes".to_string(),
                serde_json::Value::Array(
                    sorted_hashes
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }

        compute_deterministic_hash(&serde_json::Value::Object(fingerprint_map))
    }

    pub fn compute_agent_topic_metadata_hash(dto: &AgentTopicSyncDTO) -> String {
        // 排除 owner_id，仅使用 topic 自身属性计算 hash
        // 确保与桌面端 AGENT_TOPIC_SYNC_FIELDS ["id","name","createdAt","locked","unread"] 一致
        let meta = serde_json::json!({
            "id": &dto.id,
            "name": &dto.name,
            "createdAt": dto.created_at,
            "locked": dto.locked,
            "unread": dto.unread,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_group_topic_metadata_hash(dto: &GroupTopicSyncDTO) -> String {
        // 排除 owner_id，仅使用 topic 自身属性计算 hash
        // 确保与桌面端 GROUP_TOPIC_SYNC_FIELDS ["id","name","createdAt"] 一致
        let meta = serde_json::json!({
            "id": &dto.id,
            "name": &dto.name,
            "createdAt": dto.created_at,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_agent_config_hash(dto: &AgentSyncDTO) -> String {
        // 对 temperature 统一格式化到2位小数，消除 f32/f64 精度差异导致的 hash 不一致
        let meta = serde_json::json!({
            "name": &dto.name,
            "systemPrompt": &dto.system_prompt,
            "model": &dto.model,
            "temperature": (dto.temperature * 100.0).round() / 100.0,
            "contextTokenLimit": dto.context_token_limit,
            "maxOutputTokens": dto.max_output_tokens,
            "streamOutput": dto.stream_output,
        });
        compute_deterministic_hash(&meta)
    }

    pub fn compute_group_config_hash(dto: &GroupSyncDTO) -> String {
        compute_deterministic_hash(dto)
    }

    pub fn compute_avatar_hash(bytes: &[u8]) -> String {
        crate::vcp_modules::infra::utils::calculate_sha256(bytes)
    }

    pub fn compute_content_hash(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    pub async fn compute_topic_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<String, String> {
        let rows = sqlx::query(
            "SELECT content_hash FROM messages WHERE topic_id = ? AND deleted_at IS NULL ORDER BY timestamp ASC, msg_id ASC",
        )
        .bind(topic_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::with_capacity(rows.len());
        for row in rows {
            hashes.push(row.try_get("content_hash").map_err(|error| {
                format!("Topic {topic_id} message hash decode failed: {error}")
            })?);
        }
        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_agent_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(agent_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::new();
        for r in topic_rows {
            // 将 topic 的元数据 hash 和内容 hash 同时作为叶子节点，确保任何一方变动都会向上冒泡
            hashes.push(r.try_get("config_hash").map_err(|error| {
                format!("Agent {agent_id} topic config hash decode failed: {error}")
            })?);
            hashes.push(r.try_get("content_hash").map_err(|error| {
                format!("Agent {agent_id} topic content hash decode failed: {error}")
            })?);
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn compute_group_root_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<String, String> {
        let topic_rows = sqlx::query(
            "SELECT config_hash, content_hash FROM topics WHERE owner_id = ? AND owner_type = 'group' AND deleted_at IS NULL ORDER BY topic_id ASC",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut hashes = Vec::new();
        for r in topic_rows {
            hashes.push(r.try_get("config_hash").map_err(|error| {
                format!("Group {group_id} topic config hash decode failed: {error}")
            })?);
            hashes.push(r.try_get("content_hash").map_err(|error| {
                format!("Group {group_id} topic content hash decode failed: {error}")
            })?);
        }

        Ok(compute_merkle_root(hashes))
    }

    pub async fn bubble_topic_hash(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        // 1. 计算并更新 content_hash (消息聚合)
        let root_hash = Self::compute_topic_root_hash(tx, topic_id).await?;

        // 2. 计算并更新 config_hash (元数据)
        let row =
            sqlx::query("SELECT owner_type FROM topics WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(topic_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let owner_type: String = row
            .try_get("owner_type")
            .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;
        let config_hash = if owner_type == "agent" {
            let dto = HashInitializer::load_agent_topic_dto(tx, topic_id).await?;
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if owner_type == "group" {
            let dto = HashInitializer::load_group_topic_dto(tx, topic_id).await?;
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        };

        let updated =
            sqlx::query("UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(root_hash)
                .bind(config_hash)
                .bind(topic_id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Topic {topic_id} disappeared during hash update"));
        }
        Ok(())
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
        // 1. 计算并更新 content_hash (消息聚合)
        let root_hash = Self::compute_topic_root_hash(tx, topic_id).await?;

        // 2. 直接根据外部传入的元数据参数计算 config_hash (省去 2 次 SELECT)
        let config_hash = if owner_type == "agent" {
            let dto = AgentTopicSyncDTO {
                id: topic_id.to_string(),
                name: title.to_string(),
                created_at,
                locked,
                unread,
                owner_id: String::new(),
            };
            Self::compute_agent_topic_metadata_hash(&dto)
        } else if owner_type == "group" {
            let dto = GroupTopicSyncDTO {
                id: topic_id.to_string(),
                name: title.to_string(),
                created_at,
                owner_id: String::new(),
            };
            Self::compute_group_topic_metadata_hash(&dto)
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        };

        let updated =
            sqlx::query("UPDATE topics SET content_hash = ?, config_hash = ? WHERE topic_id = ? AND deleted_at IS NULL")
                .bind(root_hash)
                .bind(config_hash)
                .bind(topic_id)
                .execute(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!("Topic {topic_id} disappeared during hash update"));
        }
        Ok(())
    }

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

    pub async fn bubble_from_topic(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<(), String> {
        Self::bubble_topic_hash(tx, topic_id).await?;

        let topic_row = sqlx::query(
            "SELECT owner_id, owner_type FROM topics WHERE topic_id = ? AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let owner_id: String = topic_row
            .try_get("owner_id")
            .map_err(|error| format!("Topic {topic_id} owner id decode failed: {error}"))?;
        let owner_type: String = topic_row
            .try_get("owner_type")
            .map_err(|error| format!("Topic {topic_id} owner type decode failed: {error}"))?;

        if owner_type == "agent" {
            Self::bubble_agent_hash(tx, &owner_id).await?;
        } else if owner_type == "group" {
            Self::bubble_group_hash(tx, &owner_id).await?;
        } else {
            return Err(format!(
                "Topic {topic_id} has unsupported owner type {owner_type}"
            ));
        }

        Ok(())
    }
}

#[path = "sync_hash_initializer.rs"]
mod initializer;
pub use initializer::HashInitializer;

#[cfg(test)]
#[path = "sync_hash_tests.rs"]
mod tests;
