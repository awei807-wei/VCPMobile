use super::frame_validation::ExpectedMessages;
use super::ndjson_codec::read_response_limited;
use super::ndjson_stream::consume_stream;
use super::{BatchPullResult, MessageBatchPullRequest, PullExecutor};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync::sync_error::encode_http_sync_error_body;
use crate::vcp_modules::topic_types::TopicKey;
use reqwest::Response;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tauri::Runtime;

impl PullExecutor {
    pub async fn pull_messages_batch<R: Runtime>(
        app: &tauri::AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        requests: &[(TopicKey, Vec<String>)],
        write_queue: &DbWriteQueue,
        prerender_enabled: bool,
    ) -> Result<Vec<BatchPullResult>, String> {
        Self::pull_messages_batch_with_progress(MessageBatchPullRequest {
            app,
            client,
            http_url,
            sync_token,
            requests,
            expected_local_states: None,
            write_queue,
            prerender_enabled,
            progress: None,
        })
        .await
    }

    /// Pull messages while optionally reporting topic-level progress.
    pub async fn pull_messages_batch_with_progress<R: Runtime>(
        request: MessageBatchPullRequest<'_, R>,
    ) -> Result<Vec<BatchPullResult>, String> {
        let MessageBatchPullRequest {
            app,
            client,
            http_url,
            sync_token,
            requests,
            expected_local_states,
            write_queue,
            prerender_enabled,
            progress,
        } = request;
        let expected = validate_pull_requests(requests)?;
        let request_body = requests
            .iter()
            .map(|(topic, message_ids)| {
                json!({
                    "ownerType": topic.owner_type,
                    "ownerId": topic.owner_id,
                    "topicId": topic.topic_id,
                    "messageIds": message_ids,
                })
            })
            .collect::<Vec<_>>();
        // 桌面端在极少数情况下会长时间不产出首帧（VCP-CDS 正忙或连接假死）。
        // 只要还没有收到任何 topic，重放整批是安全的；换一个全新的连接重试一次，
        // 避免一条停滞的连接让整个 Phase 3 直接失败。
        let mut attempt = 0_u8;
        let mut retry_client: Option<reqwest::Client> = None;
        loop {
            let active_client = retry_client.as_ref().unwrap_or(client);
            let response = match request_stream(
                active_client,
                http_url,
                sync_token,
                request_body.clone(),
            )
            .await
            {
                Ok(response) => response,
                Err(error) if attempt == 0 => {
                    crate::vcp_modules::sync_service::emit_sync_log(
                        app,
                        "warn",
                        &format!(
                            "Message pull request failed ({error}); retrying once on a fresh connection"
                        ),
                    );
                    attempt = 1;
                    retry_client = Some(build_retry_client()?);
                    continue;
                }
                Err(error) => return Err(error),
            };
            match consume_stream(
                app,
                response,
                &expected,
                expected_local_states,
                write_queue,
                prerender_enabled,
                progress.clone(),
            )
            .await
            {
                Ok(results) => return Ok(results),
                Err(failure)
                    if attempt == 0 && failure.transport && failure.received_topics == 0 =>
                {
                    crate::vcp_modules::sync_service::emit_sync_log(
                        app,
                        "warn",
                        &format!(
                            "Message pull produced no topic data ({}); retrying once on a fresh connection",
                            failure.message
                        ),
                    );
                    attempt = 1;
                    retry_client = Some(build_retry_client()?);
                }
                Err(failure) => return Err(failure.message),
            }
        }
    }
}

fn build_retry_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| format!("Message pull retry client initialization failed: {error}"))
}

fn validate_pull_requests(
    requests: &[(TopicKey, Vec<String>)],
) -> Result<ExpectedMessages, String> {
    if requests.is_empty() {
        return Err("Message pull requires at least one topic".to_string());
    }
    if requests.len() > super::MAX_NDJSON_TOPICS {
        return Err(format!(
            "Message pull exceeds {} topic budget",
            super::MAX_NDJSON_TOPICS
        ));
    }
    let mut expected = HashMap::with_capacity(requests.len());
    let mut total_message_ids = 0usize;
    for (topic, message_ids) in requests {
        if !topic.is_valid() {
            return Err("Message pull requires complete TopicKey identities".to_string());
        }
        if message_ids.len() > super::MAX_MESSAGES_PER_TOPIC {
            return Err(format!(
                "Message pull for {}/{}/{} exceeds {} message budget",
                topic.owner_type,
                topic.owner_id,
                topic.topic_id,
                super::MAX_MESSAGES_PER_TOPIC
            ));
        }
        let ids = if message_ids.is_empty() {
            None
        } else {
            let mut ids = HashSet::with_capacity(message_ids.len());
            for message_id in message_ids {
                if message_id.is_empty() || !ids.insert(message_id.clone()) {
                    return Err(format!(
                        "Message pull for {}/{}/{} contains empty or duplicate message id",
                        topic.owner_type, topic.owner_id, topic.topic_id
                    ));
                }
            }
            Some(ids)
        };
        total_message_ids = total_message_ids
            .checked_add(message_ids.len())
            .ok_or_else(|| "Message pull message count overflow".to_string())?;
        if total_message_ids > super::MAX_NDJSON_ENTITIES {
            return Err(format!(
                "Message pull exceeds {} message budget",
                super::MAX_NDJSON_ENTITIES
            ));
        }
        if expected.insert(topic.clone(), ids).is_some() {
            return Err(format!(
                "Message pull contains duplicate topic identity {}/{}/{}",
                topic.owner_type, topic.owner_id, topic.topic_id
            ));
        }
    }
    Ok(expected)
}

async fn request_stream(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    topics: Vec<serde_json::Value>,
) -> Result<Response, String> {
    let response = client
        .post(format!("{http_url}/api/mobile-sync/messages/pull"))
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/json")
        .json(&json!({ "topics": topics }))
        .send()
        .await
        .map_err(|error| format!("Message pull request failed: {error}"))?;
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let (_, body) = read_response_limited(
        response,
        super::MAX_ERROR_RESPONSE_BYTES,
        "Message pull error",
    )
    .await?;
    let detail = match encode_http_sync_error_body(&body) {
        Ok(Some(error)) => error,
        Ok(None) => format!("HTTP {status} without a Wire 1.5 error object"),
        Err(error) => format!("invalid Wire 1.5 error: {error}"),
    };
    Err(format!("Message pull failed: {detail}"))
}
