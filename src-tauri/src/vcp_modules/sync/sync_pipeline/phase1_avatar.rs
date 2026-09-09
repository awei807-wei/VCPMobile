use super::{optional_timestamp, required_hash, required_timestamp};
use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, AvatarManifestDeleted, AvatarManifestLive, AvatarManifestState,
    AvatarOwnerType,
};
use sqlx::Row;
use sqlx::SqlitePool;

pub(super) async fn avatar_manifest_state(
    pool: &SqlitePool,
    row: sqlx::sqlite::SqliteRow,
) -> Result<AvatarManifestState, String> {
    let (owner_type, owner_id) = decode_avatar_identity(&row)?;
    let deleted_at = decode_avatar_deleted_at(&row, owner_type, &owner_id)?;
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
    build_live_avatar_state(&row, owner_type, owner_id)
}

fn decode_avatar_identity(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<(AvatarOwnerType, String), String> {
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
    Ok((owner_type, owner_id))
}

fn decode_avatar_deleted_at(
    row: &sqlx::sqlite::SqliteRow,
    owner_type: AvatarOwnerType,
    owner_id: &str,
) -> Result<Option<i64>, String> {
    let deleted_at: Option<i64> = row
        .try_get("deleted_at")
        .map_err(|error| format!("Avatar manifest tombstone decode failed: {error}"))?;
    optional_timestamp(
        deleted_at,
        &format!("Avatar {owner_type}/{owner_id} deletedAt"),
    )
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

fn build_live_avatar_state(
    row: &sqlx::sqlite::SqliteRow,
    owner_type: AvatarOwnerType,
    owner_id: String,
) -> Result<AvatarManifestState, String> {
    let avatar_hash: String = row.try_get("avatar_hash").map_err(|error| {
        format!("Avatar manifest hash decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    let updated_at: i64 = row
        .try_get("updated_at")
        .map_err(|error| format!("Avatar manifest timestamp decode failed: {error}"))?;
    Ok(AvatarManifestState::Live(AvatarManifestLive {
        owner_type,
        owner_id,
        binary_hash: required_hash(
            avatar_hash,
            &format!("Avatar {owner_type} binaryHash"),
            false,
        )?,
        updated_at: required_timestamp(updated_at, &format!("Avatar {owner_type} updatedAt"))?,
    }))
}
