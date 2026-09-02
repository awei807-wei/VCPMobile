use super::http::{parse_success_response, require_exact_object_keys};
use super::types::MAX_AVATAR_BYTES;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_types::is_valid_avatar_owner;
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

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
    let image_size = avatar_size(&db.pool, owner_type, owner_id).await?;
    if image_size > MAX_AVATAR_BYTES {
        return Err(format!(
            "Avatar {owner_type}/{owner_id} exceeds the {MAX_AVATAR_BYTES}-byte upload limit"
        ));
    }
    let row = sqlx::query(
        "SELECT image_data, mime_type FROM avatars
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
    )
    .bind(owner_id)
    .bind(owner_type)
    .fetch_optional(&db.pool)
    .await
    .map_err(|error| format!("Push avatar {owner_type}/{owner_id} query failed: {error}"))?
    .ok_or_else(|| format!("Avatar {owner_type}/{owner_id} is missing from the local database"))?;
    let image_data: Vec<u8> = row
        .try_get("image_data")
        .map_err(|error| format!("Avatar {owner_type}/{owner_id} image decode failed: {error}"))?;
    if image_data.len() > MAX_AVATAR_BYTES {
        return Err(format!(
            "Avatar {owner_type}/{owner_id} exceeds the {MAX_AVATAR_BYTES}-byte upload limit"
        ));
    }
    let mime_type: String = row
        .try_get("mime_type")
        .map_err(|error| format!("Avatar {owner_type}/{owner_id} MIME decode failed: {error}"))?;
    let response = client
        .post(format!(
            "{http_url}/api/mobile-sync/upload-avatar?id={}&type={}",
            urlencoding::encode(owner_id),
            urlencoding::encode(owner_type)
        ))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", mime_type)
        .body(image_data)
        .send()
        .await
        .map_err(|error| format!("Push avatar {owner_type}/{owner_id} failed: {error}"))?;
    let body = parse_success_response(response, "Push avatar").await?;
    validate_avatar_response(&body, owner_id, owner_type)
}

async fn ensure_parent_is_live(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    let parent_is_live = match owner_type {
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
    if !parent_is_live {
        return Err(format!(
            "Avatar owner {owner_type}/{owner_id} is missing or deleted"
        ));
    }
    Ok(())
}

async fn avatar_size(
    pool: &sqlx::SqlitePool,
    owner_type: &str,
    owner_id: &str,
) -> Result<usize, String> {
    let image_size: Option<i64> = sqlx::query_scalar(
        "SELECT LENGTH(image_data) FROM avatars
         WHERE owner_id = ? AND owner_type = ? AND deleted_at IS NULL",
    )
    .bind(owner_id)
    .bind(owner_type)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("Push avatar {owner_type}/{owner_id} size lookup failed: {error}"))?;
    let image_size = image_size.ok_or_else(|| {
        format!("Avatar {owner_type}/{owner_id} is missing from the local database")
    })?;
    usize::try_from(image_size)
        .map_err(|_| format!("Avatar {owner_type}/{owner_id} has an invalid image size"))
}

pub(super) fn validate_avatar_response(
    value: &serde_json::Value,
    owner_id: &str,
    owner_type: &str,
) -> Result<(), String> {
    let object = require_exact_object_keys(value, &["success", "id"], "Push avatar")?;
    if object.get("success").and_then(serde_json::Value::as_bool) != Some(true)
        || object.get("id").and_then(serde_json::Value::as_str) != Some(owner_id)
    {
        return Err(format!(
            "Push avatar response id mismatch for {owner_type}/{owner_id}"
        ));
    }
    Ok(())
}
