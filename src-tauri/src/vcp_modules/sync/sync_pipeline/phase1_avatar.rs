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
) -> Result<Option<AvatarManifestState>, String> {
    let (owner_type, owner_id) = decode_avatar_identity(&row)?;
    let deleted_at = decode_avatar_deleted_at(&row, owner_type, &owner_id)?;
    if !avatar_parent_is_live(pool, owner_type, &owner_id, deleted_at).await? {
        // 历史数据可能留下“父级已删除、头像仍是活体”的孤儿记录。
        // 构建清单前会先补墓碑；这里仅作为并发删除竞态下的兜底跳过，
        // 不能因为一条孤儿记录让整个同步失败。
        log::warn!(
            "[sync] skip orphan avatar {owner_type}/{owner_id}: parent is missing or deleted"
        );
        return Ok(None);
    }
    if let Some(deleted_at) = deleted_at {
        return Ok(Some(AvatarManifestState::Deleted(AvatarManifestDeleted {
            owner_type,
            owner_id,
            deleted_at,
        })));
    }
    build_live_avatar_state(&row, owner_type, owner_id).map(Some)
}

/// 修复历史遗留的孤儿头像：父级 Agent/Group 已删除或缺失、但头像仍是活体时，
/// 直接补一个墓碑。幂等操作，没有孤儿记录时不会产生写入。
pub(super) async fn repair_orphan_avatars(
    pool: &SqlitePool,
    deleted_at: i64,
) -> Result<u64, String> {
    let result = sqlx::query(
        "UPDATE avatars SET deleted_at = ?
         WHERE deleted_at IS NULL
           AND (
             (owner_type = 'agent' AND NOT EXISTS (
                SELECT 1 FROM agents
                WHERE agents.agent_id = avatars.owner_id AND agents.deleted_at IS NULL
             ))
             OR
             (owner_type = 'group' AND NOT EXISTS (
                SELECT 1 FROM groups
                WHERE groups.group_id = avatars.owner_id AND groups.deleted_at IS NULL
             ))
           )",
    )
    .bind(deleted_at)
    .execute(pool)
    .await
    .map_err(|error| format!("Avatar orphan repair failed: {error}"))?;
    Ok(result.rows_affected())
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
