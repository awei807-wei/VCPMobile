use super::HashInitializer;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::{Row, Sqlite, Transaction};

impl HashInitializer {
    pub async fn load_agent_topic_dto(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<AgentTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let owner_id: String = row
            .try_get("owner_id")
            .map_err(|error| format!("Agent topic {topic_id} owner decode failed: {error}"))?;
        Self::load_agent_topic_dto_for_key(tx, &TopicKey::new("agent", owner_id, topic_id)).await
    }

    pub async fn load_agent_topic_dto_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<AgentTopicSyncDTO, String> {
        if key.owner_type != "agent" || !key.is_valid() {
            return Err(format!(
                "Agent topic {} has invalid owner identity",
                key.topic_id
            ));
        }
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'agent' AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.topic_id)
        .bind(&key.owner_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentTopicSyncDTO {
            id: row.try_get("topic_id").map_err(|error| {
                format!("Agent topic {} id decode failed: {error}", key.topic_id)
            })?,
            name: row.try_get("title").map_err(|error| {
                format!("Agent topic {} title decode failed: {error}", key.topic_id)
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!(
                    "Agent topic {} created_at decode failed: {error}",
                    key.topic_id
                )
            })?,
            locked: row.try_get::<i64, _>("locked").map_err(|error| {
                format!("Agent topic {} locked decode failed: {error}", key.topic_id)
            })? != 0,
            unread: row.try_get::<i64, _>("unread").map_err(|error| {
                format!("Agent topic {} unread decode failed: {error}", key.topic_id)
            })? != 0,
            owner_id: row.try_get("owner_id").map_err(|error| {
                format!("Agent topic {} owner decode failed: {error}", key.topic_id)
            })?,
        })
    }

    pub async fn load_group_topic_dto(
        tx: &mut Transaction<'_, Sqlite>,
        topic_id: &str,
    ) -> Result<GroupTopicSyncDTO, String> {
        let row = sqlx::query(
            "SELECT owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'group' AND deleted_at IS NULL",
        )
        .bind(topic_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        let owner_id: String = row
            .try_get("owner_id")
            .map_err(|error| format!("Group topic {topic_id} owner decode failed: {error}"))?;
        Self::load_group_topic_dto_for_key(tx, &TopicKey::new("group", owner_id, topic_id)).await
    }

    pub async fn load_group_topic_dto_for_key(
        tx: &mut Transaction<'_, Sqlite>,
        key: &TopicKey,
    ) -> Result<GroupTopicSyncDTO, String> {
        if key.owner_type != "group" || !key.is_valid() {
            return Err(format!(
                "Group topic {} has invalid owner identity",
                key.topic_id
            ));
        }
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, owner_id FROM topics
             WHERE topic_id = ? AND owner_type = 'group' AND owner_id = ? AND deleted_at IS NULL",
        )
        .bind(&key.topic_id)
        .bind(&key.owner_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(GroupTopicSyncDTO {
            id: row.try_get("topic_id").map_err(|error| {
                format!("Group topic {} id decode failed: {error}", key.topic_id)
            })?,
            name: row.try_get("title").map_err(|error| {
                format!("Group topic {} title decode failed: {error}", key.topic_id)
            })?,
            created_at: row.try_get("created_at").map_err(|error| {
                format!(
                    "Group topic {} created_at decode failed: {error}",
                    key.topic_id
                )
            })?,
            owner_id: row.try_get("owner_id").map_err(|error| {
                format!("Group topic {} owner decode failed: {error}", key.topic_id)
            })?,
        })
    }
}
