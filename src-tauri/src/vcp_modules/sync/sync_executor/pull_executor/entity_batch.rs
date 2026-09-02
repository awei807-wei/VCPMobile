use super::ndjson_codec::{http_status_error, read_response_limited};
use super::PullExecutor;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::sync::sync_error::encode_wire_sync_error_value;
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use serde_json::{Map, Value};
use std::collections::HashSet;
use tauri::{AppHandle, Runtime};

const DIRECT_RESPONSE_LIMIT: usize = 10 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 1024 * 1024;

impl PullExecutor {
    pub async fn pull_entities_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: Vec<serde_json::Value>,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let expected = validate_entity_requests(&requests)?;
        let results = request_entity_batch(client, http_url, sync_token, &requests).await?;
        let (agent_topics, group_topics) =
            decode_entity_results(results, &expected, write_queue).await?;
        submit_topic_batches(write_queue, agent_topics, group_topics).await?;
        crate::vcp_modules::sync::sync_service::emit_sync_log(
            app,
            "info",
            "[PullExecutor] Batch pull completed",
        );
        Ok(())
    }
}

fn validate_entity_requests(
    requests: &[serde_json::Value],
) -> Result<HashSet<(String, String)>, String> {
    if requests.len() > super::MAX_ENTITY_BATCH_ITEMS {
        return Err(format!(
            "Entity pull request contains more than {} items",
            super::MAX_ENTITY_BATCH_ITEMS
        ));
    }
    let mut expected = HashSet::new();
    for request in requests {
        let id = request
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "Entity pull request requires a non-empty id".to_string())?;
        let entity_type = request
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "agent" | "group" | "agent_topic" | "group_topic"))
            .ok_or_else(|| format!("Entity pull request {id} has an invalid type"))?;
        if !expected.insert((id.to_string(), entity_type.to_string())) {
            return Err(format!(
                "Entity pull request contains duplicate {entity_type}/{id}"
            ));
        }
    }
    Ok(expected)
}

async fn request_entity_batch(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    requests: &[serde_json::Value],
) -> Result<Vec<Value>, String> {
    let body = serde_json::to_vec(&serde_json::json!({ "requests": requests }))
        .map_err(|error| format!("Entity pull request serialization failed: {error}"))?;
    if body.len() > DIRECT_RESPONSE_LIMIT {
        return Err("Entity pull request exceeds 10 MiB".to_string());
    }
    let response = client
        .post(format!("{http_url}/api/mobile-sync/download-entities"))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let limit = if response.status().is_success() {
        DIRECT_RESPONSE_LIMIT
    } else {
        MAX_ERROR_RESPONSE_BYTES
    };
    let (status, bytes) = read_response_limited(response, limit, "Entity pull").await?;
    if !status.is_success() {
        return Err(http_status_error("Pull entities batch", status, &bytes));
    }
    parse_strict_body(&bytes, "Entity pull")?
        .as_array()
        .cloned()
        .ok_or_else(|| "Entity pull response must be an array".to_string())
}

fn parse_strict_body(bytes: &[u8], operation: &str) -> Result<Value, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{operation} returned invalid UTF-8: {error}"))?;
    parse_strict_json(text).map_err(|error| format!("{operation} returned invalid JSON: {error}"))
}

async fn decode_entity_results(
    results: Vec<Value>,
    expected: &HashSet<(String, String)>,
    write_queue: &DbWriteQueue,
) -> Result<
    (
        Vec<(String, AgentTopicSyncDTO)>,
        Vec<(String, GroupTopicSyncDTO)>,
    ),
    String,
> {
    let mut agent_topics = Vec::new();
    let mut group_topics = Vec::new();
    let mut seen = HashSet::new();
    for item in results {
        decode_entity_result(
            item,
            expected,
            &mut seen,
            write_queue,
            &mut agent_topics,
            &mut group_topics,
        )
        .await?;
    }
    if seen != *expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!(
            "Entity pull response is missing results {missing:?}"
        ));
    }
    Ok((agent_topics, group_topics))
}

async fn decode_entity_result(
    item: Value,
    expected: &HashSet<(String, String)>,
    seen: &mut HashSet<(String, String)>,
    write_queue: &DbWriteQueue,
    agent_topics: &mut Vec<(String, AgentTopicSyncDTO)>,
    group_topics: &mut Vec<(String, GroupTopicSyncDTO)>,
) -> Result<(), String> {
    let object = item
        .as_object()
        .ok_or_else(|| "Entity pull result must be an object".to_string())?;
    validate_entity_result_fields(object)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Entity pull result requires a non-empty id".to_string())?;
    let entity_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Entity pull result {id} requires type"))?;
    let key = (id.to_string(), entity_type.to_string());
    if !expected.contains(&key) {
        return Err(format!(
            "Entity pull returned unexpected result {entity_type}/{id}"
        ));
    }
    if !seen.insert(key) {
        return Err(format!(
            "Entity pull returned duplicate result {entity_type}/{id}"
        ));
    }
    validate_entity_result_success(object, entity_type, id)?;
    let data = object
        .get("data")
        .cloned()
        .ok_or_else(|| format!("Entity pull result {entity_type}/{id} requires data"))?;
    submit_entity_result_data(
        write_queue,
        id,
        entity_type,
        data,
        agent_topics,
        group_topics,
    )
    .await
}

fn validate_entity_result_success(
    object: &Map<String, Value>,
    entity_type: &str,
    id: &str,
) -> Result<(), String> {
    if object.get("success").and_then(Value::as_bool) != Some(true) {
        let error = object
            .get("error")
            .ok_or_else(|| format!("Entity pull {entity_type}/{id} failure is missing error"))
            .and_then(encode_wire_sync_error_value)?;
        return Err(format!("Entity pull {entity_type}/{id} failed: {error}"));
    }
    if object.get("error").is_some() {
        return Err(format!(
            "Successful entity pull {entity_type}/{id} must not contain an error"
        ));
    }
    Ok(())
}

fn validate_entity_result_fields(object: &Map<String, Value>) -> Result<(), String> {
    for field in object.keys() {
        if !matches!(field.as_str(), "id" | "type" | "success" | "error" | "data") {
            return Err(format!("Entity pull result contains unknown field {field}"));
        }
    }
    Ok(())
}

async fn submit_entity_result_data(
    write_queue: &DbWriteQueue,
    id: &str,
    entity_type: &str,
    data: Value,
    agent_topics: &mut Vec<(String, AgentTopicSyncDTO)>,
    group_topics: &mut Vec<(String, GroupTopicSyncDTO)>,
) -> Result<(), String> {
    match entity_type {
        "agent" => {
            let dto = serde_json::from_value::<AgentSyncDTO>(data)
                .map_err(|error| format!("Invalid agent {id}: {error}"))?;
            write_queue
                .submit(DbWriteTask::Agent {
                    id: id.to_string(),
                    dto,
                })
                .await
        }
        "group" => {
            let dto = serde_json::from_value::<GroupSyncDTO>(data)
                .map_err(|error| format!("Invalid group {id}: {error}"))?;
            write_queue
                .submit(DbWriteTask::Group {
                    id: id.to_string(),
                    dto,
                })
                .await
        }
        "agent_topic" => {
            if id != "default" {
                let dto = serde_json::from_value::<AgentTopicSyncDTO>(data)
                    .map_err(|error| format!("Invalid agent topic {id}: {error}"))?;
                agent_topics.push((id.to_string(), dto));
            }
            Ok(())
        }
        "group_topic" => {
            if id != "default" {
                let dto = serde_json::from_value::<GroupTopicSyncDTO>(data)
                    .map_err(|error| format!("Invalid group topic {id}: {error}"))?;
                group_topics.push((id.to_string(), dto));
            }
            Ok(())
        }
        _ => Err(format!(
            "Entity pull returned unsupported type {entity_type}"
        )),
    }
}

async fn submit_topic_batches(
    write_queue: &DbWriteQueue,
    agent_topics: Vec<(String, AgentTopicSyncDTO)>,
    group_topics: Vec<(String, GroupTopicSyncDTO)>,
) -> Result<(), String> {
    if !agent_topics.is_empty() {
        write_queue
            .submit(DbWriteTask::AgentTopicBatch {
                topics: agent_topics,
            })
            .await?;
    }
    if !group_topics.is_empty() {
        write_queue
            .submit(DbWriteTask::GroupTopicBatch {
                topics: group_topics,
            })
            .await?;
    }
    Ok(())
}
