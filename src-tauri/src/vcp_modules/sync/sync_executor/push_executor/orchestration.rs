use super::attachments::upload_attachment;
use super::message_store::serialize_topic_messages;
use super::ndjson::{record_message_frames, send_message_chunk};
use super::tombstone::{
    append_topic_failure, load_message_tombstones, message_tombstone_body_len,
    preflight_topic_messages, push_message_tombstone_chunk,
};
use super::types::{
    MessageTombstone, PushBatchResult, MAX_SYNC_BODY_BYTES, MAX_SYNC_MESSAGES, MAX_SYNC_TOPICS,
    MESSAGE_REQUEST_CHUNK_BYTES,
};
use crate::vcp_modules::db_manager::DbState;
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::RwLock;

pub(super) async fn push_messages_batch<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    topic_ids: &[String],
    uploaded_hashes: Arc<RwLock<HashSet<String>>>,
) -> Result<Vec<PushBatchResult>, String> {
    validate_topic_ids(topic_ids)?;
    let db = app.state::<DbState>();
    let mut context = PushContext::default();
    for topic_id in topic_ids {
        let topic = prepare_topic(app, &db.pool, topic_id, &mut context).await?;
        context
            .append_topic(client, http_url, sync_token, topic)
            .await?;
    }
    context.flush(client, http_url, sync_token).await?;
    context
        .upload_attachments(app, client, http_url, sync_token, uploaded_hashes)
        .await?;
    log::info!(
        "[PushExecutor] Batch push completed: {}/{} topics",
        context
            .results
            .iter()
            .filter(|result| result.success)
            .count(),
        topic_ids.len()
    );
    Ok(context.results)
}

fn validate_topic_ids(topic_ids: &[String]) -> Result<(), String> {
    if topic_ids.is_empty() {
        return Ok(());
    }
    if topic_ids.len() > MAX_SYNC_TOPICS {
        return Err(format!(
            "Message push contains {} topics, limit is {}",
            topic_ids.len(),
            MAX_SYNC_TOPICS
        ));
    }
    let unique = topic_ids.iter().collect::<HashSet<_>>();
    if unique.len() != topic_ids.len() || topic_ids.iter().any(String::is_empty) {
        return Err("Message push topic ids must be unique and non-empty".to_string());
    }
    Ok(())
}

struct PreparedTopic {
    topic_id: String,
    line: Vec<u8>,
    tombstones: Vec<MessageTombstone>,
}

async fn prepare_topic<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    topic_id: &str,
    context: &mut PushContext,
) -> Result<PreparedTopic, String> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|error| format!("Message push snapshot failed for {topic_id}: {error}"))?;
    let (owner_type, owner_id) = load_topic_owner(&mut tx, topic_id).await?;
    let preflight = preflight_topic_messages(&mut tx, topic_id).await?;
    let topic_message_count = preflight
        .live_count
        .checked_add(preflight.tombstone_count)
        .ok_or_else(|| format!("Message push count overflow for {topic_id}"))?;
    context.add_message_count(topic_message_count)?;
    let tombstones = load_message_tombstones(&mut tx, topic_id, preflight.tombstone_count).await?;
    context.add_tombstone_bytes(&tombstones)?;
    let line = serialize_topic_messages(
        app,
        &mut tx,
        topic_id,
        &owner_type,
        &owner_id,
        preflight.live_count,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|error| format!("Message push snapshot close failed for {topic_id}: {error}"))?;
    context.add_line_bytes(line.len())?;
    Ok(PreparedTopic {
        topic_id: topic_id.to_string(),
        line,
        tombstones,
    })
}

async fn load_topic_owner(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    topic_id: &str,
) -> Result<(String, String), String> {
    let row = sqlx::query(
        "SELECT owner_type, owner_id FROM topics
         WHERE topic_id = ? AND deleted_at IS NULL",
    )
    .bind(topic_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| format!("Message push topic query failed: {error}"))?
    .ok_or_else(|| format!("Message push topic {topic_id} is missing locally"))?;
    let owner_type: String = row.try_get("owner_type").map_err(|error| {
        format!("Message push owner type decode failed for {topic_id}: {error}")
    })?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("Message push owner id decode failed for {topic_id}: {error}"))?;
    if !matches!(owner_type.as_str(), "agent" | "group") || owner_id.is_empty() {
        return Err(format!(
            "Message push topic {topic_id} has invalid owner identity"
        ));
    }
    Ok((owner_type, owner_id))
}

#[derive(Default)]
struct PushContext {
    results: Vec<PushBatchResult>,
    attachment_topics: HashMap<String, HashSet<String>>,
    total_request_bytes: usize,
    total_messages: usize,
    request_body: Vec<u8>,
    request_topics: Vec<String>,
    request_tombstones: Vec<MessageTombstone>,
}

impl PushContext {
    fn add_message_count(&mut self, count: usize) -> Result<(), String> {
        self.total_messages = self
            .total_messages
            .checked_add(count)
            .ok_or_else(|| "Message push count overflow".to_string())?;
        if self.total_messages > MAX_SYNC_MESSAGES {
            return Err(format!(
                "Message push contains more than {MAX_SYNC_MESSAGES} messages"
            ));
        }
        Ok(())
    }

    fn add_tombstone_bytes(&mut self, tombstones: &[MessageTombstone]) -> Result<(), String> {
        for tombstone in tombstones {
            self.total_request_bytes = self
                .total_request_bytes
                .checked_add(message_tombstone_body_len(tombstone)?)
                .ok_or_else(|| "Message push byte count overflow".to_string())?;
            self.ensure_total_budget()?;
        }
        Ok(())
    }

    fn add_line_bytes(&mut self, bytes: usize) -> Result<(), String> {
        self.total_request_bytes = self
            .total_request_bytes
            .checked_add(bytes)
            .ok_or_else(|| "Message push byte count overflow".to_string())?;
        self.ensure_total_budget()
    }

    fn ensure_total_budget(&self) -> Result<(), String> {
        if self.total_request_bytes > MAX_SYNC_BODY_BYTES {
            return Err("Message push exceeds the 256 MiB total limit".to_string());
        }
        Ok(())
    }

    async fn append_topic(
        &mut self,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic: PreparedTopic,
    ) -> Result<(), String> {
        if !self.request_body.is_empty()
            && self.request_body.len().saturating_add(topic.line.len())
                > MESSAGE_REQUEST_CHUNK_BYTES
        {
            self.flush(client, http_url, sync_token).await?;
        }
        self.request_tombstones.extend(topic.tombstones);
        if self.request_body.is_empty() {
            self.request_body = topic.line;
        } else {
            self.request_body.extend_from_slice(&topic.line);
        }
        self.request_topics.push(topic.topic_id);
        if self.request_body.len() >= MESSAGE_REQUEST_CHUNK_BYTES {
            self.flush(client, http_url, sync_token).await?;
        }
        Ok(())
    }

    async fn flush(
        &mut self,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
    ) -> Result<(), String> {
        if self.request_body.is_empty() {
            return Ok(());
        }
        let body = std::mem::take(&mut self.request_body);
        let topics = std::mem::take(&mut self.request_topics);
        let frames = send_message_chunk(client, http_url, sync_token, body, &topics).await?;
        record_message_frames(frames, &mut self.results, &mut self.attachment_topics);
        let tombstones = std::mem::take(&mut self.request_tombstones);
        push_message_tombstone_chunk(client, http_url, sync_token, tombstones, &mut self.results)
            .await
    }

    async fn upload_attachments<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    ) -> Result<(), String> {
        let result_indexes = self
            .results
            .iter()
            .enumerate()
            .map(|(index, result)| (result.topic_id.clone(), index))
            .collect::<HashMap<_, _>>();
        let hashes = {
            let tracker = uploaded_hashes.read().await;
            self.attachment_topics
                .keys()
                .filter(|hash| !tracker.contains(*hash))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut failures = HashMap::new();
        let request = AttachmentUploadRequest {
            app,
            client,
            http_url,
            sync_token,
            uploaded_hashes: &uploaded_hashes,
        };
        for chunk in hashes.chunks(3) {
            self.upload_chunk(&request, chunk, &mut failures).await;
        }
        self.apply_attachment_failures(failures, &result_indexes)
    }

    async fn upload_chunk<R: Runtime>(
        &self,
        request: &AttachmentUploadRequest<'_, R>,
        hashes: &[String],
        failures: &mut HashMap<String, String>,
    ) {
        let futures = hashes.iter().map(|hash| {
            upload_attachment(
                request.app,
                request.client,
                request.http_url,
                request.sync_token,
                hash,
            )
        });
        for (hash, outcome) in hashes
            .iter()
            .zip(futures_util::future::join_all(futures).await)
        {
            match outcome {
                Ok(()) => {
                    request.uploaded_hashes.write().await.insert(hash.clone());
                }
                Err(error) => {
                    failures.insert(hash.clone(), error);
                }
            }
        }
    }

    fn apply_attachment_failures(
        &mut self,
        failures: HashMap<String, String>,
        result_indexes: &HashMap<String, usize>,
    ) -> Result<(), String> {
        for (hash, error) in failures {
            if let Some(topics) = self.attachment_topics.get(&hash) {
                for topic_id in topics {
                    let index = result_indexes.get(topic_id).copied().ok_or_else(|| {
                        format!("Attachment result references missing topic {topic_id}")
                    })?;
                    append_topic_failure(
                        &mut self.results[index],
                        format!("Attachment {hash} required by topic {topic_id} failed: {error}"),
                    );
                }
            }
        }
        Ok(())
    }
}

struct AttachmentUploadRequest<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    client: &'a reqwest::Client,
    http_url: &'a str,
    sync_token: &'a str,
    uploaded_hashes: &'a Arc<RwLock<HashSet<String>>>,
}
