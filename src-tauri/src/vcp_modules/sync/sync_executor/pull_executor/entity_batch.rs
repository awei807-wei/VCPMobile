use super::ndjson_codec::{http_status_error, read_response_limited};
use super::PullExecutor;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_types::OwnerType;
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::collections::HashSet;
use tauri::{AppHandle, Emitter, Runtime};

mod entity_results;

const ENTITY_REQUEST_LIMIT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum EntityIdentity {
    Owner {
        owner_type: OwnerType,
        owner_id: String,
    },
    Topic(TopicKey),
}

impl EntityIdentity {
    fn label(&self) -> String {
        match self {
            Self::Owner {
                owner_type,
                owner_id,
            } => format!("owner/{owner_type}/{owner_id}"),
            Self::Topic(key) => {
                format!("topic/{}/{}/{}", key.owner_type, key.owner_id, key.topic_id)
            }
        }
    }
}

#[derive(Debug)]
struct ValidEntityRequest {
    identity: EntityIdentity,
}

#[derive(Debug)]
struct DecodedEntityResult {
    identity: EntityIdentity,
    task: Option<DbWriteTask>,
    error: Option<String>,
}

impl PullExecutor {
    pub async fn pull_entities_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: Vec<Value>,
        write_queue: &DbWriteQueue,
        owner_config_baselines: HashMap<OwnerKey, String>,
    ) -> Result<(), String> {
        let expected = parse_entity_requests(&requests)?;
        let body = serde_json::to_vec(&serde_json::json!({ "items": requests }))
            .map_err(|error| format!("Entity pull request serialization failed: {error}"))?;
        if body.len() > ENTITY_REQUEST_LIMIT_BYTES {
            return Err("Entity pull request exceeds 10 MiB".to_string());
        }
        let response = client
            .post(format!("{http_url}/api/mobile-sync/entities/pull"))
            .header("Authorization", format!("Bearer {sync_token}"))
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| format!("Entity pull request failed: {error}"))?;
        let limit = if response.status().is_success() {
            ENTITY_REQUEST_LIMIT_BYTES
        } else {
            super::MAX_ERROR_RESPONSE_BYTES
        };
        let (status, bytes) = read_response_limited(response, limit, "Entity pull").await?;
        if !status.is_success() {
            return Err(http_status_error("Entity pull", status, &bytes));
        }
        let response = parse_entity_response(&bytes)?;
        let decoded =
            entity_results::decode_entity_results(response, &expected, &owner_config_baselines)?;
        let mut failures = Vec::new();
        for item in decoded {
            if let Some(error) = item.error {
                failures.push(format!("{}: {error}", item.identity.label()));
                continue;
            }
            if let Some(task) = item.task {
                write_queue.submit(task).await?;
            }
        }
        if !failures.is_empty() {
            return Err(format!(
                "Entity pull item failures: {}",
                failures.join(" | ")
            ));
        }
        let _ = app.emit(
            "vcp-sync-log",
            serde_json::json!({
                "level": "info",
                "message": "[PullExecutor] Wire 1.4 entity pull completed"
            }),
        );
        Ok(())
    }
}

fn parse_entity_requests(values: &[Value]) -> Result<Vec<ValidEntityRequest>, String> {
    if values.is_empty() {
        return Err("Entity pull requires at least one selector".to_string());
    }
    if values.len() > super::MAX_ENTITY_BATCH_ITEMS {
        return Err(format!(
            "Entity pull request contains more than {} items",
            super::MAX_ENTITY_BATCH_ITEMS
        ));
    }
    let mut seen = HashSet::new();
    let mut requests = Vec::with_capacity(values.len());
    for value in values {
        let identity = parse_entity_identity(value)?;
        if !seen.insert(identity.clone()) {
            return Err(format!(
                "Entity pull request contains duplicate {}",
                identity.label()
            ));
        }
        requests.push(ValidEntityRequest { identity });
    }
    Ok(requests)
}

fn parse_entity_identity(value: &Value) -> Result<EntityIdentity, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Entity selector must be an object".to_string())?;
    let entity_type = object
        .get("entityType")
        .and_then(Value::as_str)
        .ok_or_else(|| "Entity selector requires entityType".to_string())?;
    match entity_type {
        "owner" => {
            require_exact_keys(object, &["entityType", "ownerType", "ownerId"])?;
            let owner_type = parse_owner_type(object.get("ownerType"))?;
            let owner_id = non_empty_string(object.get("ownerId"), "ownerId")?;
            Ok(EntityIdentity::Owner {
                owner_type,
                owner_id,
            })
        }
        "topic" => {
            require_exact_keys(object, &["entityType", "ownerType", "ownerId", "topicId"])?;
            let owner_type = parse_owner_type(object.get("ownerType"))?;
            let owner_id = non_empty_string(object.get("ownerId"), "ownerId")?;
            let topic_id = non_empty_string(object.get("topicId"), "topicId")?;
            Ok(EntityIdentity::Topic(TopicKey::new(
                owner_type.as_str(),
                owner_id,
                topic_id,
            )))
        }
        other => Err(format!(
            "Entity selector has unsupported entityType {other}"
        )),
    }
}

fn parse_entity_response(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Entity pull response returned invalid UTF-8: {error}"))?;
    let value = parse_strict_json(text)
        .map_err(|error| format!("Entity pull response returned invalid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "Entity pull response must be an object".to_string())?;
    require_exact_keys(object, &["results"])?;
    let results = object
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| "Entity pull response results must be an array".to_string())?;
    if results.len() > super::MAX_ENTITY_BATCH_ITEMS {
        return Err(format!(
            "Entity pull response exceeds {} results",
            super::MAX_ENTITY_BATCH_ITEMS
        ));
    }
    Ok(results.clone())
}

fn parse_owner_type(value: Option<&Value>) -> Result<OwnerType, String> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| "ownerType must be a string".to_string())?;
    OwnerType::try_from(value).map_err(|_| format!("unsupported ownerType {value}"))
}

fn non_empty_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

fn require_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> Result<(), String> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        let mut actual = object.keys().cloned().collect::<Vec<_>>();
        actual.sort();
        let mut expected = expected
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<_>>();
        expected.sort();
        return Err(format!(
            "object fields mismatch: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}
