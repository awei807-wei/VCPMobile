use super::super::HashAggregator;
use super::HashInitializer;
use crate::vcp_modules::sync_dto::AgentSyncDTO;
use sqlx::{Row, Sqlite, Transaction};

impl HashInitializer {
    pub async fn ensure_agent_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let row =
            sqlx::query("SELECT config_hash FROM agents WHERE agent_id = ? AND deleted_at IS NULL")
                .bind(agent_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let r = row.ok_or_else(|| format!("Agent {agent_id} is missing or deleted"))?;
        let config_hash: Option<String> = r
            .try_get("config_hash")
            .map_err(|error| format!("Agent {agent_id} hash decode failed: {error}"))?;
        if config_hash
            .as_deref()
            .is_none_or(|hash| hash.is_empty() || hash == "PENDING")
        {
            Self::recompute_agent_config_hash(tx, agent_id).await?;
        }

        Ok(())
    }

    pub(super) async fn recompute_agent_config_hash(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<(), String> {
        let dto = Self::load_agent_dto(tx, agent_id).await?;
        let new_hash = HashAggregator::compute_agent_config_hash(&dto);
        let updated = sqlx::query(
            "UPDATE agents SET config_hash = ? WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(&new_hash)
        .bind(agent_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Agent {agent_id} disappeared during hash initialization"
            ));
        }
        log::debug!(
            "[HashInitializer] Initialized config_hash for Agent {}",
            agent_id
        );
        Ok(())
    }

    pub async fn ensure_all_agent_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let rows = sqlx::query(
            "SELECT agent_id FROM agents
             WHERE deleted_at IS NULL
               AND (config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING')",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        for row in rows {
            let agent_id: String = row
                .try_get("agent_id")
                .map_err(|error| format!("Agent hash id decode failed: {error}"))?;
            Self::ensure_agent_hashes(&mut tx, &agent_id)
                .await
                .map_err(|error| {
                    format!("Failed to initialize hash for Agent {agent_id}: {error}")
                })?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        log::info!("[HashInitializer] Ensured all Agent hashes");
        Ok(())
    }

    async fn load_agent_dto(
        tx: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<AgentSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, system_prompt, model, temperature, context_token_limit, max_output_tokens, stream_output
             FROM agents WHERE agent_id = ? AND deleted_at IS NULL",
        )
        .bind(agent_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        Ok(AgentSyncDTO {
            name: row
                .try_get("name")
                .map_err(|error| format!("Agent {agent_id} name decode failed: {error}"))?,
            system_prompt: row.try_get("system_prompt").map_err(|error| {
                format!("Agent {agent_id} system prompt decode failed: {error}")
            })?,
            model: row
                .try_get("model")
                .map_err(|error| format!("Agent {agent_id} model decode failed: {error}"))?,
            temperature: row
                .try_get("temperature")
                .map_err(|error| format!("Agent {agent_id} temperature decode failed: {error}"))?,
            context_token_limit: row.try_get("context_token_limit").map_err(|error| {
                format!("Agent {agent_id} context token limit decode failed: {error}")
            })?,
            max_output_tokens: row.try_get("max_output_tokens").map_err(|error| {
                format!("Agent {agent_id} max output tokens decode failed: {error}")
            })?,
            stream_output: row.try_get::<i64, _>("stream_output").map_err(|error| {
                format!("Agent {agent_id} stream output decode failed: {error}")
            })? != 0,
        })
    }
}
