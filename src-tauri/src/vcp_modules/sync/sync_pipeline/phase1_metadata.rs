use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, AvatarManifestDeleted, AvatarManifestLive, AvatarManifestState,
    AvatarOwnerType, ManifestRequest, OwnerManifestDeleted, OwnerManifestLive, OwnerManifestState,
    OwnerType, TopicManifestDeleted, TopicManifestLive, TopicManifestState, MAX_MANIFEST_ITEMS,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashSet;

#[path = "phase1_topic_fields.rs"]
mod topic_fields;
use topic_fields::decode_topic_live_fields;

const SQLITE_BIND_CHUNK: usize = 400;
const MAX_SAFE_TIMESTAMP: i64 = 9_007_199_254_740_991;

pub struct Phase1Metadata;

fn valid_sha256(value: &str, allow_empty: bool) -> bool {
    (allow_empty && value.is_empty())
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn required_hash(value: String, label: &str, allow_empty: bool) -> Result<String, String> {
    if !valid_sha256(&value, allow_empty) {
        return Err(format!("{label} must be a lowercase SHA-256 hash"));
    }
    Ok(value)
}

fn required_timestamp(value: i64, label: &str) -> Result<i64, String> {
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&value) {
        return Err(format!("{label} must be a non-negative safe integer"));
    }
    Ok(value)
}

fn optional_timestamp(value: Option<i64>, label: &str) -> Result<Option<i64>, String> {
    value
        .map(|value| required_timestamp(value, label))
        .transpose()
}

fn validate_count(count: usize, label: &str) -> Result<(), String> {
    if count > MAX_MANIFEST_ITEMS {
        return Err(format!("{label} exceeds the item budget"));
    }
    Ok(())
}

fn validate_targeted_owners(owners: &[OwnerKey]) -> Result<HashSet<OwnerKey>, String> {
    validate_count(owners.len(), "targetedOwners")?;
    let expected = owners.iter().cloned().collect::<HashSet<_>>();
    if expected.len() != owners.len() || expected.iter().any(|owner| !owner.is_valid()) {
        return Err("targetedOwners requires unique agent/group identities".into());
    }
    Ok(expected)
}

fn owner_manifest_state(row: &sqlx::sqlite::SqliteRow) -> Result<OwnerManifestState, String> {
    let raw_owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Owner manifest type decode failed: {error}"))?;
    let owner_type = OwnerType::try_from(raw_owner_type.as_str())
        .map_err(|_| format!("Owner manifest has unsupported owner type {raw_owner_type}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Owner manifest id decode failed: {error}"))?;
    if owner_id.is_empty() {
        return Err("Owner manifest has an empty owner id".to_string());
    }
    let deleted_at: Option<i64> = row
        .try_get("deleted_at")
        .map_err(|error| format!("Owner manifest tombstone decode failed: {error}"))?;
    let deleted_at = optional_timestamp(
        deleted_at,
        &format!("Owner {owner_type}/{owner_id} deletedAt"),
    )?;
    if let Some(deleted_at) = deleted_at {
        return Ok(OwnerManifestState::Deleted(OwnerManifestDeleted {
            owner_type,
            owner_id,
            deleted_at,
        }));
    }
    let config_hash: String = row.try_get("config_hash").map_err(|error| {
        format!("Owner manifest config hash decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    let content_hash: String = row.try_get("content_hash").map_err(|error| {
        format!("Owner manifest content hash decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    Ok(OwnerManifestState::Live(OwnerManifestLive {
        owner_type,
        owner_id,
        config_hash: required_hash(
            config_hash,
            &format!("Owner {owner_type} configHash"),
            false,
        )?,
        content_hash: required_hash(
            content_hash,
            &format!("Owner {owner_type} contentHash"),
            true,
        )?,
        updated_at: required_timestamp(
            row.try_get("updated_at")
                .map_err(|error| format!("Owner manifest timestamp decode failed: {error}"))?,
            &format!("Owner {owner_type} updatedAt"),
        )?,
    }))
}

fn topic_manifest_state(
    row: &sqlx::sqlite::SqliteRow,
    expected_owners: &HashSet<OwnerKey>,
) -> Result<TopicManifestState, String> {
    let (owner_type, owner_id, topic_id) = decode_topic_identity(row, expected_owners)?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(|error| {
        format!("Topic manifest tombstone decode failed for {owner_type}/{owner_id}/{topic_id}: {error}")
    })?;
    let deleted_at = optional_timestamp(
        deleted_at,
        &format!("Topic {owner_type}/{owner_id}/{topic_id} deletedAt"),
    )?;
    if let Some(deleted_at) = deleted_at {
        return Ok(TopicManifestState::Deleted(TopicManifestDeleted {
            owner_type,
            owner_id,
            topic_id,
            deleted_at,
        }));
    }
    let (config_hash, content_hash, updated_at) =
        decode_topic_live_fields(row, &owner_type, &owner_id, &topic_id)?;
    Ok(TopicManifestState::Live(TopicManifestLive {
        owner_type,
        owner_id,
        topic_id,
        config_hash: required_hash(
            config_hash,
            &format!("Topic {owner_type} configHash"),
            false,
        )?,
        content_hash: required_hash(
            content_hash,
            &format!("Topic {owner_type} contentHash"),
            true,
        )?,
        updated_at: required_timestamp(updated_at, &format!("Topic {owner_type} updatedAt"))?,
    }))
}

fn decode_topic_identity(
    row: &sqlx::sqlite::SqliteRow,
    expected_owners: &HashSet<OwnerKey>,
) -> Result<(OwnerType, String, String), String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("Topic manifest id decode failed: {error}"))?;
    if topic_id.is_empty() {
        return Err("Topic manifest has an empty topic id".to_string());
    }
    let raw_owner_type: String = row.try_get("owner_type").map_err(|error| {
        format!("Topic manifest owner type decode failed for {topic_id}: {error}")
    })?;
    let owner_type = OwnerType::try_from(raw_owner_type.as_str()).map_err(|_| {
        format!("Topic manifest {topic_id} has unsupported owner type {raw_owner_type}")
    })?;
    let owner_id: String = row.try_get("owner_id").map_err(|error| {
        format!("Topic manifest owner id decode failed for {topic_id}: {error}")
    })?;
    let owner = OwnerKey::new(owner_type.as_str(), &owner_id);
    if !owner.is_valid() || !expected_owners.contains(&owner) {
        return Err(format!(
            "Topic manifest {topic_id} returned unexpected owner {owner_type}/{owner_id}"
        ));
    }
    let key = TopicKey::new(owner_type.as_str(), &owner_id, &topic_id);
    if !key.is_valid() {
        return Err(format!("Topic manifest returned invalid identity {key:?}"));
    }
    Ok((owner_type, owner_id, topic_id))
}

async fn avatar_parent_is_live(
    pool: &SqlitePool,
    owner_type: AvatarOwnerType,
    owner_id: &str,
    deleted_at: Option<i64>,
) -> Result<bool, String> {
    if deleted_at.is_some() || owner_type == AvatarOwnerType::User {
        return Ok(true);
    }
    let (table, column) = match owner_type {
        AvatarOwnerType::Agent => ("agents", "agent_id"),
        AvatarOwnerType::Group => ("groups", "group_id"),
        AvatarOwnerType::User => unreachable!("user avatars return above"),
    };
    let sql =
        format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} = ? AND deleted_at IS NULL)");
    sqlx::query_scalar::<_, i64>(&sql)
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .map(|value| value != 0)
        .map_err(|error| {
            format!("Avatar manifest {owner_type} owner lookup failed for {owner_id}: {error}")
        })
}

async fn avatar_manifest_state(
    pool: &SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> Result<AvatarManifestState, String> {
    let raw_owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("Avatar manifest owner type decode failed: {error}"))?;
    let owner_type = AvatarOwnerType::try_from(raw_owner_type.as_str())
        .map_err(|_| format!("Avatar manifest has invalid owner {raw_owner_type}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Avatar manifest owner id decode failed: {error}"))?;
    if !is_valid_avatar_owner(owner_type.as_str(), &owner_id) {
        return Err(format!(
            "Avatar manifest has invalid owner {owner_type}/{owner_id}"
        ));
    }
    let deleted_at: Option<i64> = row
        .try_get("deleted_at")
        .map_err(|error| format!("Avatar manifest tombstone decode failed: {error}"))?;
    let deleted_at = optional_timestamp(
        deleted_at,
        &format!("Avatar {owner_type}/{owner_id} deletedAt"),
    )?;
    if !avatar_parent_is_live(pool, owner_type, &owner_id, deleted_at).await? {
        return Err(format!(
            "Avatar manifest owner {owner_type}/{owner_id} is missing or deleted"
        ));
    }
    if let Some(deleted_at) = deleted_at {
        return Ok(AvatarManifestState::Deleted(AvatarManifestDeleted {
            owner_type,
            owner_id,
            deleted_at,
        }));
    }
    let avatar_hash: String = row.try_get("avatar_hash").map_err(|error| {
        format!("Avatar manifest hash decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    Ok(AvatarManifestState::Live(AvatarManifestLive {
        owner_type,
        owner_id,
        binary_hash: required_hash(
            avatar_hash,
            &format!("Avatar {owner_type} binaryHash"),
            false,
        )?,
        updated_at: required_timestamp(
            row.try_get("updated_at")
                .map_err(|error| format!("Avatar manifest timestamp decode failed: {error}"))?,
            &format!("Avatar {owner_type} updatedAt"),
        )?,
    }))
}

impl Phase1Metadata {
    pub async fn build_owner_manifest(pool: &SqlitePool) -> Result<ManifestRequest, String> {
        let rows = sqlx::query(
            "SELECT 'agent' AS owner_type, agent_id AS owner_id, config_hash, content_hash,
                    updated_at, deleted_at
             FROM agents
             UNION ALL
             SELECT 'group' AS owner_type, group_id AS owner_id, config_hash, content_hash,
                    updated_at, deleted_at
             FROM groups
             ORDER BY owner_type, owner_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("Owner manifest query failed: {error}"))?;
        validate_count(rows.len(), "owner manifest")?;
        let items = rows
            .iter()
            .map(owner_manifest_state)
            .collect::<Result<Vec<_>, _>>()?;
        let manifest = ManifestRequest::Owner { items };
        manifest.validate()?;
        Ok(manifest)
    }

    pub async fn build_targeted_topic_manifest(
        pool: &SqlitePool,
        owners: &[OwnerKey],
    ) -> Result<ManifestRequest, String> {
        let expected_owners = validate_targeted_owners(owners)?;
        if owners.is_empty() {
            return Ok(ManifestRequest::Topic {
                items: Vec::new(),
                targeted_owners: Vec::new(),
            });
        }
        let mut items = Vec::new();
        let mut seen_topics = HashSet::new();
        for owner_chunk in owners.chunks(SQLITE_BIND_CHUNK) {
            let predicates = owner_chunk
                .iter()
                .map(|_| "(owner_type = ? AND owner_id = ?)")
                .collect::<Vec<_>>()
                .join(" OR ");
            let query_sql = format!(
                "SELECT topic_id, config_hash, content_hash, updated_at, owner_type, owner_id, deleted_at
                 FROM topics WHERE {predicates}
                 ORDER BY owner_type, owner_id, topic_id"
            );
            let mut query = sqlx::query(&query_sql);
            for owner in owner_chunk {
                query = query.bind(&owner.owner_type).bind(&owner.owner_id);
            }
            for row in query
                .fetch_all(pool)
                .await
                .map_err(|error| format!("Topic manifest query failed: {error}"))?
            {
                let state = topic_manifest_state(&row, &expected_owners)?;
                let identity = match &state {
                    TopicManifestState::Live(value) => {
                        TopicKey::new(value.owner_type.as_str(), &value.owner_id, &value.topic_id)
                    }
                    TopicManifestState::Deleted(value) => {
                        TopicKey::new(value.owner_type.as_str(), &value.owner_id, &value.topic_id)
                    }
                };
                if !seen_topics.insert(identity.clone()) {
                    return Err(format!(
                        "Targeted topic manifest returned duplicate topic {identity:?}"
                    ));
                }
                items.push(state);
            }
        }
        validate_count(items.len(), "topic manifest")?;
        let manifest = ManifestRequest::Topic {
            items,
            targeted_owners: owners.to_vec(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub async fn build_avatar_manifest(pool: &SqlitePool) -> Result<ManifestRequest, String> {
        let rows = sqlx::query(
            "SELECT av.owner_id, av.owner_type, av.avatar_hash, av.updated_at, av.deleted_at,
                    CASE
                        WHEN av.deleted_at IS NOT NULL THEN 1
                        WHEN av.owner_type = 'user' THEN 1
                        WHEN av.owner_type = 'agent' AND agent.agent_id IS NOT NULL THEN 1
                        WHEN av.owner_type = 'group' AND owner_group.group_id IS NOT NULL THEN 1
                        ELSE 0
                    END AS parent_is_live
             FROM avatars av
             LEFT JOIN agents agent
               ON av.owner_type = 'agent'
              AND agent.agent_id = av.owner_id
              AND agent.deleted_at IS NULL
             LEFT JOIN groups owner_group
               ON av.owner_type = 'group'
              AND owner_group.group_id = av.owner_id
              AND owner_group.deleted_at IS NULL
             ORDER BY av.owner_type, av.owner_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("Avatar manifest query failed: {error}"))?;
        validate_count(rows.len(), "avatar manifest")?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(avatar_manifest_state(pool, row).await?);
        }
        let manifest = ManifestRequest::Avatar { items };
        manifest.validate()?;
        Ok(manifest)
    }
}
#[cfg(test)]
#[path = "phase1_metadata_tests.rs"]
mod tests;
