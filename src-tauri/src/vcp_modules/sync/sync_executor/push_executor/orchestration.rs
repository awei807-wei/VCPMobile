use super::message_store::serialize_topic_messages;
use super::ndjson::send_message_chunk;
use super::tombstone::preflight_topic_messages;
use super::types::{
    PushBatchResult, MAX_SYNC_BODY_BYTES, MAX_SYNC_MESSAGES, MAX_SYNC_TOPICS,
    MESSAGE_REQUEST_CHUNK_BYTES,
};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::SqlitePool;
use std::collections::HashSet;
use tauri::{AppHandle, Manager, Runtime};

pub(super) async fn push_messages_batch<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    topics: &[TopicKey],
) -> Result<Vec<PushBatchResult>, String> {
    validate_topics(topics)?;
    if topics.is_empty() {
        return Ok(Vec::new());
    }
    let db = app.state::<DbState>();
    let mut state = PushState::default();
    for topic in topics {
        let (line, message_count) = snapshot_topic(&db.pool, topic).await?;
        if !state.body.is_empty()
            && state.body.len().saturating_add(line.len()) > MESSAGE_REQUEST_CHUNK_BYTES
        {
            state.flush(client, http_url, sync_token).await?;
        }
        state.add_topic(topic, line, message_count)?;
        if state.body.len() >= MESSAGE_REQUEST_CHUNK_BYTES {
            state.flush(client, http_url, sync_token).await?;
        }
    }
    state.flush(client, http_url, sync_token).await?;
    if state.results.len() != topics.len() {
        return Err("Message push response did not cover every requested topic".to_string());
    }
    log::info!(
        "[PushExecutor] Wire 1.4 message push completed: {}/{} topics",
        state.results.iter().filter(|result| result.success).count(),
        topics.len()
    );
    Ok(state.results)
}

fn validate_topics(topics: &[TopicKey]) -> Result<(), String> {
    if topics.len() > MAX_SYNC_TOPICS {
        return Err(format!(
            "Message push contains {} topics, limit is {MAX_SYNC_TOPICS}",
            topics.len()
        ));
    }
    let identities = topics.iter().cloned().collect::<HashSet<_>>();
    if identities.len() != topics.len() || topics.iter().any(|topic| !topic.is_valid()) {
        return Err("Message push topics must have unique valid identities".to_string());
    }
    Ok(())
}

async fn snapshot_topic(pool: &SqlitePool, topic: &TopicKey) -> Result<(Vec<u8>, usize), String> {
    let mut tx = pool.begin().await.map_err(|error| {
        format!(
            "Message push snapshot failed for {}: {error}",
            topic.topic_id
        )
    })?;
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM topics
            WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL
        )",
    )
    .bind(&topic.owner_type)
    .bind(&topic.owner_id)
    .bind(&topic.topic_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| {
        format!(
            "Message push topic query failed for {}: {error}",
            topic.topic_id
        )
    })?;
    if !exists {
        return Err(format!(
            "Message push topic {}/{}/{} is missing locally",
            topic.owner_type, topic.owner_id, topic.topic_id
        ));
    }
    let preflight = preflight_topic_messages(&mut tx, topic).await?;
    let line = serialize_topic_messages(&mut tx, topic).await?;
    let serialized_count = line
        .live_count
        .checked_add(line.tombstone_count)
        .ok_or_else(|| format!("Message push count overflow for {}", topic.topic_id))?;
    let expected_count = preflight
        .live_count
        .checked_add(preflight.tombstone_count)
        .ok_or_else(|| format!("Message push count overflow for {}", topic.topic_id))?;
    if serialized_count != expected_count {
        return Err(format!(
            "Message push topic {} changed during serialization: expected {expected_count}, got {serialized_count}",
            topic.topic_id
        ));
    }
    tx.commit().await.map_err(|error| {
        format!(
            "Message push snapshot close failed for {}: {error}",
            topic.topic_id
        )
    })?;
    Ok((line.line, serialized_count))
}

#[derive(Default)]
struct PushState {
    body: Vec<u8>,
    topics: Vec<TopicKey>,
    results: Vec<PushBatchResult>,
    total_bytes: usize,
    total_messages: usize,
}

impl PushState {
    fn add_topic(
        &mut self,
        topic: &TopicKey,
        line: Vec<u8>,
        message_count: usize,
    ) -> Result<(), String> {
        self.total_bytes = self
            .total_bytes
            .checked_add(line.len())
            .ok_or_else(|| "Message push byte count overflow".to_string())?;
        if self.total_bytes > MAX_SYNC_BODY_BYTES {
            return Err("Message push exceeds the 256 MiB total limit".to_string());
        }
        self.total_messages = self
            .total_messages
            .checked_add(message_count)
            .ok_or_else(|| "Message push count overflow".to_string())?;
        if self.total_messages > MAX_SYNC_MESSAGES {
            return Err(format!(
                "Message push contains more than {MAX_SYNC_MESSAGES} messages"
            ));
        }
        self.body.extend_from_slice(&line);
        self.topics.push(topic.clone());
        Ok(())
    }

    async fn flush(
        &mut self,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
    ) -> Result<(), String> {
        if self.body.is_empty() {
            return Ok(());
        }
        let body = std::mem::take(&mut self.body);
        let topics = std::mem::take(&mut self.topics);
        let frames = send_message_chunk(client, http_url, sync_token, body, &topics).await?;
        self.results
            .extend(frames.into_iter().map(|frame| frame.outcome));
        Ok(())
    }
}
