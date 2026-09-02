use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, EntityState, SyncDataType, SyncManifest,
};
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashSet;

const SQLITE_BIND_CHUNK: usize = 400;

pub struct Phase1Metadata;

fn validate_target_owners(owners: &[String]) -> Result<HashSet<String>, String> {
    let expected = owners.iter().cloned().collect::<HashSet<_>>();
    if expected.len() != owners.len() || expected.iter().any(|id| id.is_empty()) {
        return Err("Targeted topic manifest contains empty or duplicate owner ids".into());
    }
    Ok(expected)
}

fn decode_topic_state(
    row: sqlx::sqlite::SqliteRow,
    expected_owners: &HashSet<String>,
) -> Result<Option<EntityState>, String> {
    let id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Topic manifest id decode failed: {error}"))?;
    if id == "default" {
        return Ok(None);
    }
    let config_hash: String = row
        .try_get("config_hash")
        .map_err(|error| format!("Topic manifest config hash decode failed for {id}: {error}"))?;
    let content_hash: String = row
        .try_get("content_hash")
        .map_err(|error| format!("Topic manifest content hash decode failed for {id}: {error}"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Topic manifest owner type decode failed for {id}: {error}"))?;
    if !matches!(owner_type.as_str(), "agent" | "group") {
        return Err(format!(
            "Topic manifest {id} has unsupported owner type {owner_type}"
        ));
    }
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Topic manifest owner id decode failed for {id}: {error}"))?;
    if owner_id.is_empty() || !expected_owners.contains(&owner_id) {
        return Err(format!(
            "Topic manifest {id} returned unexpected owner {owner_id}"
        ));
    }
    Ok(Some(EntityState {
        id,
        hash: config_hash.clone(),
        config_hash: Some(config_hash),
        content_hash: Some(content_hash),
        ts: row
            .try_get("updated_at")
            .map_err(|error| format!("Topic manifest timestamp decode failed: {error}"))?,
        deleted_at: row
            .try_get("deleted_at")
            .map_err(|error| format!("Topic manifest tombstone decode failed: {error}"))?,
        owner_type: Some(owner_type),
        owner_id: Some(owner_id),
    }))
}

async fn load_targeted_topics(
    pool: &SqlitePool,
    owners: &[String],
    expected: &HashSet<String>,
) -> Result<Vec<EntityState>, String> {
    let mut items = Vec::new();
    let mut seen_topics = HashSet::new();
    for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = owner_chunk
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, owner_id, deleted_at \
             FROM topics WHERE owner_id IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql);
        for owner_id in owner_chunk {
            query = query.bind(owner_id);
        }
        for row in query
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?
        {
            let Some(item) = decode_topic_state(row, expected)? else {
                continue;
            };
            if !seen_topics.insert(item.id.clone()) {
                return Err(format!(
                    "Targeted topic manifest returned duplicate topic {}",
                    item.id
                ));
            }
            items.push(item);
        }
    }
    Ok(items)
}

async fn avatar_parent_is_live(
    pool: &SqlitePool,
    owner_type: &str,
    owner_id: &str,
    deleted_at: Option<i64>,
) -> Result<bool, String> {
    if deleted_at.is_some() || owner_type == "user" {
        return Ok(true);
    }
    let (table, column) = match owner_type {
        "agent" => ("agents", "agent_id"),
        "group" => ("groups", "group_id"),
        _ => return Ok(false),
    };
    let sql =
        format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} = ? AND deleted_at IS NULL)");
    sqlx::query_scalar(&sql)
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .map_err(|error| {
            format!("Avatar manifest {owner_type} owner lookup failed for {owner_id}: {error}")
        })
}

async fn decode_avatar_state(
    pool: &SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> Result<EntityState, String> {
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Avatar manifest owner type decode failed: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Avatar manifest owner id decode failed: {error}"))?;
    if !is_valid_avatar_owner(&owner_type, &owner_id) {
        return Err(format!(
            "Avatar manifest has invalid owner {owner_type}/{owner_id}"
        ));
    }
    let deleted_at = row
        .try_get("deleted_at")
        .map_err(|error| format!("Avatar manifest tombstone decode failed: {error}"))?;
    if !avatar_parent_is_live(pool, &owner_type, &owner_id, deleted_at).await? {
        return Err(format!(
            "Avatar manifest owner {owner_type}/{owner_id} is missing or deleted"
        ));
    }
    Ok(EntityState {
        id: format!("{owner_type}:{owner_id}"),
        hash: row.try_get("avatar_hash").map_err(|error| {
            format!("Avatar manifest hash decode failed for {owner_type}/{owner_id}: {error}")
        })?,
        config_hash: None,
        content_hash: None,
        ts: row
            .try_get("updated_at")
            .map_err(|error| format!("Avatar manifest timestamp decode failed: {error}"))?,
        deleted_at,
        owner_type: None,
        owner_id: None,
    })
}

impl Phase1Metadata {
    pub async fn build_agent_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT agent_id, config_hash, content_hash, updated_at, deleted_at 
             FROM agents",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r
                .try_get("agent_id")
                .map_err(|error| format!("Agent manifest id decode failed: {error}"))?;
            let conf_h: String = r.try_get("config_hash").map_err(|error| {
                format!("Agent manifest config hash decode failed for {id}: {error}")
            })?;
            let cont_h: String = r.try_get("content_hash").map_err(|error| {
                format!("Agent manifest content hash decode failed for {id}: {error}")
            })?;
            items.push(EntityState {
                id,
                hash: conf_h.clone(), // 兼容旧版，默认使用 config_hash
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Agent manifest timestamp decode failed: {error}"))?,
                deleted_at: r
                    .try_get("deleted_at")
                    .map_err(|error| format!("Agent manifest tombstone decode failed: {error}"))?,
                owner_type: None,
                owner_id: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Agent,
            items,
        })
    }

    pub async fn build_group_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT group_id, config_hash, content_hash, updated_at, deleted_at 
             FROM groups",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::new();
        for r in rows {
            let id: String = r
                .try_get("group_id")
                .map_err(|error| format!("Group manifest id decode failed: {error}"))?;
            let conf_h: String = r.try_get("config_hash").map_err(|error| {
                format!("Group manifest config hash decode failed for {id}: {error}")
            })?;
            let cont_h: String = r.try_get("content_hash").map_err(|error| {
                format!("Group manifest content hash decode failed for {id}: {error}")
            })?;
            items.push(EntityState {
                id,
                hash: conf_h.clone(),
                config_hash: Some(conf_h),
                content_hash: Some(cont_h),
                ts: r
                    .try_get("updated_at")
                    .map_err(|error| format!("Group manifest timestamp decode failed: {error}"))?,
                deleted_at: r
                    .try_get("deleted_at")
                    .map_err(|error| format!("Group manifest tombstone decode failed: {error}"))?,
                owner_type: None,
                owner_id: None,
            });
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Group,
            items,
        })
    }

    pub async fn build_targeted_topic_manifest(
        pool: &SqlitePool,
        owners: &[String],
    ) -> Result<SyncManifest, String> {
        if owners.is_empty() {
            return Ok(SyncManifest {
                data_type: SyncDataType::Topic,
                items: Vec::new(),
            });
        }

        let expected_owners = validate_target_owners(owners)?;
        let items = load_targeted_topics(pool, owners, &expected_owners).await?;

        Ok(SyncManifest {
            data_type: SyncDataType::Topic,
            items,
        })
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<SyncManifest, String> {
        let rows = sqlx::query(
            "SELECT owner_id, owner_type, avatar_hash, updated_at, deleted_at
             FROM avatars",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(decode_avatar_state(pool, row).await?);
        }

        Ok(SyncManifest {
            data_type: SyncDataType::Avatar,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Phase1Metadata;

    #[tokio::test]
    async fn avatar_manifest_preserves_tombstones() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE avatars (
                owner_id TEXT, owner_type TEXT, avatar_hash TEXT,
                updated_at INTEGER, deleted_at INTEGER
             );
             INSERT INTO avatars VALUES
                ('agent-a', 'agent', 'hash', 10, 9),
                ('user_avatar', 'user', 'user-hash', 11, NULL);",
        )
        .execute(&pool)
        .await
        .expect("create avatar fixture");

        let manifest = Phase1Metadata::build_avatar_manifest(&pool)
            .await
            .expect("build avatar manifest");
        assert_eq!(manifest.items.len(), 2);
        assert_eq!(manifest.items[0].id, "agent:agent-a");
        assert_eq!(manifest.items[0].deleted_at, Some(9));
        assert_eq!(manifest.items[1].id, "user:user_avatar");
        assert_eq!(manifest.items[1].deleted_at, None);
    }

    #[tokio::test]
    async fn targeted_topic_manifest_carries_exact_owner_identity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open database");
        sqlx::query(
            "CREATE TABLE topics (
                topic_id TEXT, config_hash TEXT, content_hash TEXT,
                updated_at INTEGER, owner_type TEXT, owner_id TEXT,
                deleted_at INTEGER
             );
             INSERT INTO topics VALUES
                ('topic-a', 'config-hash', 'content-hash', 10,
                 'agent', 'agent-a', NULL);",
        )
        .execute(&pool)
        .await
        .expect("create topic fixture");

        let manifest =
            Phase1Metadata::build_targeted_topic_manifest(&pool, &["agent-a".to_string()])
                .await
                .expect("build topic manifest");
        assert_eq!(manifest.items.len(), 1);
        assert_eq!(manifest.items[0].owner_type.as_deref(), Some("agent"));
        assert_eq!(manifest.items[0].owner_id.as_deref(), Some("agent-a"));
    }
}
