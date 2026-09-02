use super::ndjson_codec::read_response_limited;
use super::ndjson_stream::consume_stream;
use super::{BatchPullResult, PullExecutor, PullProgressContext};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync::sync_error::encode_http_sync_error_body;
use crate::vcp_modules::sync::sync_executor::pull_executor::MAX_ERROR_RESPONSE_BYTES;
use crate::vcp_modules::sync::sync_executor::pull_executor::MAX_MESSAGE_PULL_TOPICS;
use crate::vcp_modules::sync::sync_executor::pull_executor::MAX_NDJSON_ENTITIES;
use crate::vcp_modules::sync::sync_executor::pull_executor::SQLITE_BIND_CHUNK;
use reqwest::Response;
use serde_json::Value;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Manager, Runtime};

type ExpectedMessages = HashMap<String, Option<HashSet<String>>>;
type TopicIdentities = HashMap<String, (String, String)>;

pub struct PullBatchContext<'a, R: Runtime> {
    pub app: &'a AppHandle<R>,
    pub client: &'a reqwest::Client,
    pub http_url: &'a str,
    pub sync_token: &'a str,
    pub requests: &'a [(String, Vec<String>)],
    pub write_queue: &'a DbWriteQueue,
    pub prerender_enabled: bool,
    pub progress: Option<PullProgressContext>,
}

async fn pull_messages_batch_impl<R: Runtime>(
    context: PullBatchContext<'_, R>,
) -> Result<Vec<BatchPullResult>, String> {
    if context.requests.is_empty() {
        return Ok(Vec::new());
    }
    let expected_messages = validate_pull_requests(context.requests)?;
    let identities = load_topic_identities(context.app, context.requests).await?;
    let request_body = build_request_body(context.requests, &identities)?;
    let response = request_stream(
        context.client,
        context.http_url,
        context.sync_token,
        request_body,
    )
    .await?;
    consume_stream(
        context.app,
        response,
        &expected_messages,
        &identities,
        context.write_queue,
        context.prerender_enabled,
        context.progress,
    )
    .await
}

fn validate_pull_requests(requests: &[(String, Vec<String>)]) -> Result<ExpectedMessages, String> {
    if requests.len() > MAX_MESSAGE_PULL_TOPICS {
        return Err(format!(
            "Pull request exceeds {MAX_MESSAGE_PULL_TOPICS} topic budget"
        ));
    }
    let mut expected = HashMap::new();
    let mut total_message_ids = 0usize;
    for (topic_id, message_ids) in requests {
        if topic_id.is_empty() || expected.contains_key(topic_id) {
            return Err("Pull request contains empty or duplicate topicId".to_string());
        }
        if message_ids.len() > super::MAX_ENTITY_BATCH_ITEMS {
            return Err(format!(
                "Pull request for {topic_id} exceeds {} message budget",
                super::MAX_ENTITY_BATCH_ITEMS
            ));
        }
        total_message_ids = total_message_ids
            .checked_add(message_ids.len())
            .ok_or_else(|| "Pull request message count overflow".to_string())?;
        if total_message_ids > MAX_NDJSON_ENTITIES {
            return Err(format!(
                "Pull request exceeds {MAX_NDJSON_ENTITIES} message budget"
            ));
        }
        expected.insert(topic_id.clone(), exact_message_ids(topic_id, message_ids)?);
    }
    Ok(expected)
}

fn exact_message_ids(
    topic_id: &str,
    message_ids: &[String],
) -> Result<Option<HashSet<String>>, String> {
    if message_ids.is_empty() {
        return Ok(None);
    }
    let ids = message_ids.iter().cloned().collect::<HashSet<_>>();
    if ids.len() != message_ids.len() || ids.iter().any(|id| id.is_empty()) {
        return Err(format!(
            "Pull request for {topic_id} contains empty or duplicate message id"
        ));
    }
    Ok(Some(ids))
}

async fn load_topic_identities<R: Runtime>(
    app: &AppHandle<R>,
    requests: &[(String, Vec<String>)],
) -> Result<TopicIdentities, String> {
    let db = app.state::<DbState>();
    let mut identities = HashMap::new();
    for chunk in requests.chunks(SQLITE_BIND_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let query_text = format!(
            "SELECT topic_id, owner_type, owner_id FROM topics
             WHERE topic_id IN ({placeholders}) AND deleted_at IS NULL"
        );
        let mut query = sqlx::query(&query_text);
        for (topic_id, _) in chunk {
            query = query.bind(topic_id);
        }
        let rows = query
            .fetch_all(&db.pool)
            .await
            .map_err(|error| format!("Pull topic identity lookup failed: {error}"))?;
        decode_topic_identity_rows(rows, &mut identities)?;
    }
    ensure_all_topic_identities(requests, &identities)
}

fn decode_topic_identity_rows(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    identities: &mut TopicIdentities,
) -> Result<(), String> {
    for row in rows {
        let topic_id = row
            .try_get::<String, _>("topic_id")
            .map_err(|error| format!("Pull topic id decode failed: {error}"))?;
        let owner_type = row
            .try_get::<String, _>("owner_type")
            .map_err(|error| format!("Pull topic {topic_id} owner type decode failed: {error}"))?;
        let owner_id = row
            .try_get::<String, _>("owner_id")
            .map_err(|error| format!("Pull topic {topic_id} owner id decode failed: {error}"))?;
        if !matches!(owner_type.as_str(), "agent" | "group") || owner_id.is_empty() {
            return Err(format!("Pull topic {topic_id} has invalid owner identity"));
        }
        if identities
            .insert(topic_id.clone(), (owner_type, owner_id))
            .is_some()
        {
            return Err(format!(
                "Pull topic identity query returned duplicate topic {topic_id}"
            ));
        }
    }
    Ok(())
}

fn ensure_all_topic_identities(
    requests: &[(String, Vec<String>)],
    identities: &TopicIdentities,
) -> Result<TopicIdentities, String> {
    if identities.len() == requests.len() {
        return Ok(identities.clone());
    }
    let requested = requests
        .iter()
        .map(|(topic_id, _)| topic_id)
        .collect::<HashSet<_>>();
    let mut missing = requested
        .iter()
        .filter(|topic_id| !identities.contains_key(**topic_id))
        .map(|topic_id| (*topic_id).clone())
        .collect::<Vec<_>>();
    missing.sort();
    Err(format!(
        "Pull topic identity lookup omitted topics: {:?}",
        missing.into_iter().take(8).collect::<Vec<_>>()
    ))
}

fn build_request_body(
    requests: &[(String, Vec<String>)],
    identities: &TopicIdentities,
) -> Result<Vec<Value>, String> {
    requests
        .iter()
        .map(|(topic_id, message_ids)| {
            let (owner_type, owner_id) = identities
                .get(topic_id)
                .ok_or_else(|| format!("Pull topic {topic_id} identity disappeared"))?;
            Ok(serde_json::json!({
                "topicId": topic_id,
                "ownerType": owner_type,
                "ownerId": owner_id,
                "msgIds": message_ids,
            }))
        })
        .collect()
}

async fn request_stream(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    request_body: Vec<Value>,
) -> Result<Response, String> {
    let response = client
        .post(format!(
            "{http_url}/api/mobile-sync/download-messages-stream"
        ))
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .json(&serde_json::json!({ "requests": request_body }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let (_, body) =
        read_response_limited(response, MAX_ERROR_RESPONSE_BYTES, "Batch pull error").await?;
    Err(match encode_http_sync_error_body(&body) {
        Ok(Some(error)) => error,
        Ok(None) => {
            format!("Batch pull messages failed with HTTP {status} without a Wire 1.2 error object")
        }
        Err(error) => format!("Batch pull messages returned an invalid Wire 1.2 error: {error}"),
    })
}

impl PullExecutor {
    pub async fn pull_messages_batch<R: Runtime>(
        context: PullBatchContext<'_, R>,
    ) -> Result<Vec<BatchPullResult>, String> {
        pull_messages_batch_impl(context).await
    }
}
