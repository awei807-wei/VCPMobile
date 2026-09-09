use super::http::{http_transport_error, parse_json_response};
use crate::vcp_modules::agent_service;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync::sync_error::{encode_wire_sync_error, SyncErrorStage};
use crate::vcp_modules::sync::sync_hash::HashAggregator;
use crate::vcp_modules::sync::sync_types::{
    EntityPushData, EntityPushItem, EntityPushRequest, EntityPushResponse, EntitySelector,
    OwnerType,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;
use tauri::{AppHandle, Manager, Runtime};

const ENTITY_REQUEST_LIMIT_BYTES: usize = 10 * 1024 * 1024;
const OWNER_PUSH_IDEMPOTENCY_DOMAIN: &[u8] = b"VCPMobileSync.OwnerPush.Idempotency.v1";

pub(super) async fn push_agent<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    agent_id: &str,
) -> Result<(), String> {
    let config =
        agent_service::read_agent_config_internal(app, &app.state(), agent_id, None).await?;
    let dto = crate::vcp_modules::sync_dto::AgentSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_agent_config_hash(&dto);
    let version = load_owner_push_version(
        &app.state::<crate::vcp_modules::db_manager::DbState>().pool,
        OwnerType::Agent,
        agent_id,
        &config_hash,
    )
    .await?;
    send_entity_items(
        client,
        http_url,
        sync_token,
        vec![EntityPushItem::Owner {
            owner_type: OwnerType::Agent,
            owner_id: agent_id.to_string(),
            data: EntityPushData::Agent(dto),
        }],
        Some(owner_push_idempotency_key(
            OwnerType::Agent,
            agent_id,
            &config_hash,
            version,
        )),
    )
    .await
}

pub(super) async fn push_group<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    group_id: &str,
) -> Result<(), String> {
    let config =
        group_service::read_group_config(app.clone(), app.state(), group_id.to_string()).await?;
    let dto = crate::vcp_modules::sync_dto::GroupSyncDTO::from(&config);
    let config_hash = HashAggregator::compute_group_config_hash(&dto);
    let version = load_owner_push_version(
        &app.state::<crate::vcp_modules::db_manager::DbState>().pool,
        OwnerType::Group,
        group_id,
        &config_hash,
    )
    .await?;
    send_entity_items(
        client,
        http_url,
        sync_token,
        vec![EntityPushItem::Owner {
            owner_type: OwnerType::Group,
            owner_id: group_id.to_string(),
            data: EntityPushData::Group(dto),
        }],
        Some(owner_push_idempotency_key(
            OwnerType::Group,
            group_id,
            &config_hash,
            version,
        )),
    )
    .await
}

pub(super) async fn push_entities_batch(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    items: Vec<EntityPushItem>,
) -> Result<(), String> {
    send_entity_items(client, http_url, sync_token, items, None).await
}

pub(super) async fn load_owner_push_version(
    pool: &sqlx::SqlitePool,
    owner_type: OwnerType,
    owner_id: &str,
    expected_config_hash: &str,
) -> Result<i64, String> {
    let query = match owner_type {
        OwnerType::Agent => {
            "SELECT config_hash, updated_at FROM agents
             WHERE agent_id = ? AND deleted_at IS NULL"
        }
        OwnerType::Group => {
            "SELECT config_hash, updated_at FROM groups
             WHERE group_id = ? AND deleted_at IS NULL"
        }
    };
    let row = sqlx::query(query)
        .bind(owner_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            format!("Owner push version query failed for {owner_type}/{owner_id}: {error}")
        })?
        .ok_or_else(|| format!("Owner {owner_type}/{owner_id} is unavailable for push"))?;
    let stored_hash: String = row.try_get("config_hash").map_err(|error| {
        format!("Owner config hash decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    if stored_hash != expected_config_hash {
        return Err(format!(
            "Owner {owner_type}/{owner_id} changed while preparing its push snapshot"
        ));
    }
    let version: i64 = row.try_get("updated_at").map_err(|error| {
        format!("Owner update version decode failed for {owner_type}/{owner_id}: {error}")
    })?;
    if version < 0 {
        return Err(format!(
            "Owner {owner_type}/{owner_id} has an invalid negative update version"
        ));
    }
    Ok(version)
}

async fn send_entity_items(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    items: Vec<EntityPushItem>,
    idempotency_key: Option<String>,
) -> Result<(), String> {
    let request = EntityPushRequest { items };
    request.validate()?;
    let expected = request
        .items
        .iter()
        .map(EntityPushItem::selector)
        .collect::<HashSet<_>>();
    let body = serde_json::to_vec(&request)
        .map_err(|error| format!("Entity push serialization failed: {error}"))?;
    if body.len() > ENTITY_REQUEST_LIMIT_BYTES {
        return Err("Entity push request exceeds 10 MiB".to_string());
    }
    let stage = if expected
        .iter()
        .any(|selector| matches!(selector, EntitySelector::Topic { .. }))
    {
        SyncErrorStage::TopicMetadata
    } else {
        SyncErrorStage::OwnerMetadata
    };
    let mut builder = client
        .post(format!("{http_url}/api/mobile-sync/entities/push"))
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("x-idempotency-key", key);
    }
    let response = builder
        .body(body)
        .send()
        .await
        .map_err(|error| http_transport_error("Entity push request", stage, &error))?;
    let response: EntityPushResponse = parse_json_response(response, "Entity push", stage).await?;
    validate_entity_response(response, &expected)
}

fn validate_entity_response(
    response: EntityPushResponse,
    expected: &HashSet<EntitySelector>,
) -> Result<(), String> {
    response.validate()?;
    let mut seen = HashSet::new();
    for result in response.results {
        let (identity, ok, error) = result.into_parts();
        if !expected.contains(&identity) {
            return Err(format!(
                "Entity push returned unexpected {}",
                identity.label()
            ));
        }
        if !seen.insert(identity.clone()) {
            return Err(format!(
                "Entity push returned duplicate {}",
                identity.label()
            ));
        }
        match (ok, error) {
            (true, None) => {}
            (true, Some(_)) => {
                return Err(format!(
                    "Successful entity push {} must not contain error",
                    identity.label()
                ));
            }
            (false, Some(error)) => {
                let encoded = encode_wire_sync_error(&error)?;
                return Err(format!(
                    "Entity push {} failed: {encoded}",
                    identity.label()
                ));
            }
            (false, None) => {
                return Err(format!(
                    "Entity push {} failure requires error",
                    identity.label()
                ));
            }
        }
    }
    if seen != *expected {
        return Err("Entity push response is missing one or more requested identities".to_string());
    }
    Ok(())
}

pub(super) fn owner_push_idempotency_key(
    owner_type: OwnerType,
    owner_id: &str,
    config_hash: &str,
    updated_at: i64,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(OWNER_PUSH_IDEMPOTENCY_DOMAIN);
    for field in [
        owner_type.as_str().as_bytes(),
        owner_id.as_bytes(),
        config_hash.as_bytes(),
        updated_at.to_be_bytes().as_slice(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}
