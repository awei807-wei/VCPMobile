use super::super::HashAggregator;
use super::HashInitializer;
use crate::vcp_modules::sync_dto::GroupSyncDTO;
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

impl HashInitializer {
    pub async fn ensure_group_hashes(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let row =
            sqlx::query("SELECT config_hash FROM groups WHERE group_id = ? AND deleted_at IS NULL")
                .bind(group_id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|e| e.to_string())?;

        let r = row.ok_or_else(|| format!("Group {group_id} is missing or deleted"))?;
        let config_hash: Option<String> = r
            .try_get("config_hash")
            .map_err(|error| format!("Group {group_id} hash decode failed: {error}"))?;
        if config_hash
            .as_deref()
            .is_none_or(|hash| hash.is_empty() || hash == "PENDING")
        {
            Self::recompute_group_config_hash(tx, group_id).await?;
        }

        Ok(())
    }

    pub(super) async fn recompute_group_config_hash(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<(), String> {
        let dto = Self::load_group_dto(tx, group_id).await?;
        let new_hash = HashAggregator::compute_group_config_hash(&dto);
        let updated = sqlx::query(
            "UPDATE groups SET config_hash = ? WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(&new_hash)
        .bind(group_id)
        .execute(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "Group {group_id} disappeared during hash initialization"
            ));
        }
        log::debug!(
            "[HashInitializer] Initialized config_hash for Group {}",
            group_id
        );
        Ok(())
    }

    pub async fn ensure_all_group_hashes(pool: &sqlx::SqlitePool) -> Result<(), String> {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        let rows = sqlx::query(
            "SELECT group_id FROM groups
             WHERE deleted_at IS NULL
               AND (config_hash = '' OR config_hash IS NULL OR config_hash = 'PENDING')",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        for row in rows {
            let group_id: String = row
                .try_get("group_id")
                .map_err(|error| format!("Group hash id decode failed: {error}"))?;
            Self::ensure_group_hashes(&mut tx, &group_id)
                .await
                .map_err(|error| {
                    format!("Failed to initialize hash for Group {group_id}: {error}")
                })?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;

        log::info!("[HashInitializer] Ensured all Group hashes");
        Ok(())
    }

    async fn load_group_dto(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<GroupSyncDTO, String> {
        let row = sqlx::query(
            "SELECT name, mode, group_prompt, invite_prompt, use_unified_model, unified_model, tag_match_mode, created_at
             FROM groups WHERE group_id = ? AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        let members = Self::load_group_members(tx, group_id).await?;
        let member_tags = Self::load_member_tags(tx, group_id).await?;

        Ok(GroupSyncDTO {
            name: row
                .try_get("name")
                .map_err(|error| format!("Group {group_id} name decode failed: {error}"))?,
            members,
            mode: row
                .try_get("mode")
                .map_err(|error| format!("Group {group_id} mode decode failed: {error}"))?,
            member_tags: Some(member_tags),
            group_prompt: row
                .try_get("group_prompt")
                .map_err(|error| format!("Group {group_id} prompt decode failed: {error}"))?,
            invite_prompt: row.try_get("invite_prompt").map_err(|error| {
                format!("Group {group_id} invite prompt decode failed: {error}")
            })?,
            use_unified_model: row
                .try_get::<i64, _>("use_unified_model")
                .map_err(|error| {
                    format!("Group {group_id} unified model flag decode failed: {error}")
                })?
                != 0,
            unified_model: row.try_get("unified_model").map_err(|error| {
                format!("Group {group_id} unified model decode failed: {error}")
            })?,
            tag_match_mode: row.try_get("tag_match_mode").map_err(|error| {
                format!("Group {group_id} tag match mode decode failed: {error}")
            })?,
            created_at: row
                .try_get("created_at")
                .map_err(|error| format!("Group {group_id} created_at decode failed: {error}"))?,
        })
    }

    async fn load_group_members(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<Vec<String>, String> {
        let rows = sqlx::query(
            "SELECT agent_id FROM group_members WHERE group_id = ? ORDER BY sort_order",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                row.try_get("agent_id").map_err(|error| {
                    format!("Group {group_id} member agent id decode failed: {error}")
                })
            })
            .collect()
    }

    async fn load_member_tags(
        tx: &mut Transaction<'_, Sqlite>,
        group_id: &str,
    ) -> Result<Value, String> {
        let rows = sqlx::query(
            "SELECT agent_id, member_tag FROM group_members WHERE group_id = ? AND member_tag IS NOT NULL",
        )
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;

        let mut tags = serde_json::Map::new();
        for row in rows {
            let agent_id: String = row.try_get("agent_id").map_err(|error| {
                format!("Group {group_id} member tag agent id decode failed: {error}")
            })?;
            let tag: String = row
                .try_get("member_tag")
                .map_err(|error| format!("Group {group_id} member tag decode failed: {error}"))?;
            tags.insert(agent_id, Value::String(tag));
        }

        Ok(Value::Object(tags))
    }
}
