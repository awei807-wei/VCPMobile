use super::http::{http_transport_error, parse_json_response};
use super::types::MAX_AVATAR_BYTES;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync::sync_error::SyncErrorStage;
use crate::vcp_modules::sync::sync_types::{
    is_valid_avatar_owner, AvatarOwnerType, AvatarPushResponse,
};
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

async fn load_avatar_payload(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
) -> Result<(Vec<u8>, String), String> {
    let row = sqlx::query(
        "SELECT image_data, mime_type FROM avatars
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
    )
    .bind(owner_id)
    .bind(owner_type)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("Push avatar {owner_type}/{owner_id} query failed: {error}"))?
    .ok_or_else(|| format!("Avatar {owner_type}/{owner_id} is missing from the local database"))?;
    let image_data: Vec<u8> = row.try_get("image_data").map_err(|error| {
        format!("Push avatar {owner_type}/{owner_id} image decode failed: {error}")
    })?;
    if image_data.len() > MAX_AVATAR_BYTES {
        return Err(format!(
            "Avatar {owner_type}/{owner_id} exceeds the {MAX_AVATAR_BYTES}-byte upload limit"
        ));
    }
    let mime_type: String = row.try_get("mime_type").map_err(|error| {
        format!("Push avatar {owner_type}/{owner_id} MIME decode failed: {error}")
    })?;
    Ok((image_data, mime_type))
}

fn validate_avatar_response(
    response: AvatarPushResponse,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    response.validate()?;
    let expected_type = AvatarOwnerType::try_from(owner_type)
        .map_err(|_| format!("Invalid avatar owner type {owner_type}"))?;
    if !response.ok || response.owner_type != expected_type || response.owner_id != owner_id {
        return Err(format!(
            "Push avatar response identity mismatch for {owner_type}/{owner_id}"
        ));
    }
    Ok(())
}

pub(super) async fn push_avatar<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    if !is_valid_avatar_owner(owner_type, owner_id) {
        return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
    }
    let db = app.state::<DbState>();
    ensure_parent_is_live(&db.pool, owner_type, owner_id).await?;
    let (image_data, mime_type) = load_avatar_payload(&db.pool, owner_type, owner_id).await?;
    let url = format!(
        "{http_url}/api/mobile-sync/avatars/push?ownerType={}&ownerId={}",
        urlencoding::encode(owner_type),
        urlencoding::encode(owner_id)
    );
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", mime_type)
        .body(image_data)
        .send()
        .await
        .map_err(|error| {
            http_transport_error(
                &format!("Push avatar {owner_type}/{owner_id}"),
                SyncErrorStage::OwnerMetadata,
                &error,
            )
        })?;
    let response: AvatarPushResponse =
        parse_json_response(response, "Push avatar", SyncErrorStage::OwnerMetadata).await?;
    validate_avatar_response(response, owner_type, owner_id)
}

async fn ensure_parent_is_live(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    let live = match owner_type {
        "agent" => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE agent_id = ? AND deleted_at IS NULL)",
            )
            .bind(owner_id)
            .fetch_one(pool)
            .await
        }
        "group" => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM groups WHERE group_id = ? AND deleted_at IS NULL)",
            )
            .bind(owner_id)
            .fetch_one(pool)
            .await
        }
        "user" => Ok(true),
        _ => Ok(false),
    }
    .map_err(|error| format!("Push avatar owner lookup failed: {error}"))?;
    if live {
        Ok(())
    } else {
        Err(format!(
            "Avatar owner {owner_type}/{owner_id} is missing or deleted"
        ))
    }
}
