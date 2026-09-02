use super::http::{parse_success_response, require_exact_object_keys};
use super::types::generate_idempotency_key;
use crate::vcp_modules::agent_service;
use crate::vcp_modules::group_service;
use crate::vcp_modules::sync_dto::{AgentSyncDTO, GroupSyncDTO};
use crate::vcp_modules::sync_error::encode_wire_sync_error_value;
use std::collections::HashSet;
use tauri::{AppHandle, Manager, Runtime};

pub(super) async fn push_agent<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    agent_id: &str,
) -> Result<(), String> {
    let config =
        agent_service::read_agent_config_internal(app, &app.state(), agent_id, None).await?;
    let dto = AgentSyncDTO::from(&config);
    let body = send_entity(client, http_url, sync_token, agent_id, "agent", dto).await?;
    validate_identity_response(&body, agent_id, "Push agent")
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
    let dto = GroupSyncDTO::from(&config);
    let body = send_entity(client, http_url, sync_token, group_id, "group", dto).await?;
    validate_identity_response(&body, group_id, "Push group")
}

async fn send_entity<T: serde::Serialize>(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    id: &str,
    entity_type: &str,
    data: T,
) -> Result<serde_json::Value, String> {
    let idempotency_key = generate_idempotency_key("push", entity_type, id);
    let response = client
        .post(format!("{http_url}/api/mobile-sync/upload-entity"))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("x-idempotency-key", idempotency_key)
        .header("Content-Type", "application/json")
        .body(serialize_entity_request(id, entity_type, data)?)
        .send()
        .await
        .map_err(|error| format!("Push {entity_type} {id} request failed: {error}"))?;
    parse_success_response(response, &format!("Push {entity_type}")).await
}

fn serialize_entity_request<T: serde::Serialize>(
    id: &str,
    entity_type: &str,
    data: T,
) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(&serde_json::json!({
        "id": id,
        "type": entity_type,
        "data": data
    }))
    .map_err(|error| format!("Push {entity_type} request serialization failed: {error}"))?;
    if body.len() > 5 * 1024 * 1024 {
        return Err(format!("Push {entity_type} request exceeds 5 MiB"));
    }
    Ok(body)
}

pub(super) fn validate_identity_response(
    value: &serde_json::Value,
    expected_id: &str,
    operation: &str,
) -> Result<(), String> {
    let object = require_exact_object_keys(value, &["success", "id"], operation)?;
    if object.get("success").and_then(serde_json::Value::as_bool) != Some(true)
        || object.get("id").and_then(serde_json::Value::as_str) != Some(expected_id)
    {
        return Err(format!(
            "{operation} response id mismatch for {expected_id}"
        ));
    }
    Ok(())
}

pub(super) async fn push_entities_batch<R: Runtime>(
    _app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    items: Vec<serde_json::Value>,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let expected_ids = validate_entity_items(&items)?;
    let request_body = serde_json::json!({ "items": items });
    let request_size = serde_json::to_vec(&request_body)
        .map_err(|error| format!("Batch push entity serialization failed: {error}"))?
        .len();
    if request_size > 10 * 1024 * 1024 {
        return Err("Batch push entity request exceeds 10 MiB".to_string());
    }
    let response = client
        .post(format!("{http_url}/api/mobile-sync/upload-entities-batch"))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .json(&request_body)
        .send()
        .await
        .map_err(|error| format!("Batch push request failed: {error}"))?;
    let response_body =
        super::http::parse_success_response(response, "Batch push entities").await?;
    validate_batch_response(&response_body, &expected_ids)
}

fn validate_entity_items(items: &[serde_json::Value]) -> Result<HashSet<String>, String> {
    let mut expected_ids = HashSet::new();
    for item in items {
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Batch push entity item requires a non-empty id".to_string())?;
        if !expected_ids.insert(id.to_string()) {
            return Err(format!(
                "Batch push entity request contains duplicate id {id}"
            ));
        }
    }
    Ok(expected_ids)
}

fn validate_batch_response(
    response_body: &serde_json::Value,
    expected_ids: &HashSet<String>,
) -> Result<(), String> {
    let object = require_exact_object_keys(
        response_body,
        &["success", "results"],
        "Batch push entities",
    )?;
    if object.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Batch push entities response must report success=true".to_string());
    }
    let results = object
        .get("results")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Batch push entities response results must be an array".to_string())?;
    let mut seen_ids = HashSet::new();
    for result in results {
        validate_batch_result(result, expected_ids, &mut seen_ids)?;
    }
    if seen_ids != *expected_ids {
        let mut missing = expected_ids
            .difference(&seen_ids)
            .cloned()
            .collect::<Vec<_>>();
        missing.sort();
        return Err(format!(
            "Batch push entities missing results for {missing:?}"
        ));
    }
    Ok(())
}

fn validate_batch_result(
    result: &serde_json::Value,
    expected_ids: &HashSet<String>,
    seen_ids: &mut HashSet<String>,
) -> Result<(), String> {
    let object = result
        .as_object()
        .ok_or_else(|| "Batch push entity result must be an object".to_string())?;
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Batch push entity result requires a non-empty id".to_string())?;
    if !expected_ids.contains(id) {
        return Err(format!("Batch push entities returned unexpected id {id}"));
    }
    if !seen_ids.insert(id.to_string()) {
        return Err(format!("Batch push entities returned duplicate id {id}"));
    }
    let success = object
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| format!("Batch push entity {id} requires boolean success"))?;
    let allowed = if success {
        &["id", "success"][..]
    } else {
        &["id", "success", "error"][..]
    };
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(format!(
            "Batch push entity {id} response has unexpected fields"
        ));
    }
    if !success {
        let error = object
            .get("error")
            .ok_or_else(|| format!("Batch push entity {id} failure is missing error"))
            .and_then(encode_wire_sync_error_value)?;
        return Err(format!("Batch push entity {id} failed: {error}"));
    }
    Ok(())
}
