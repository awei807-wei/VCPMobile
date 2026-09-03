use super::PullExecutor;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync::sync_types::OwnerType;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;
use sqlx::Row;
use tauri::{AppHandle, Manager, Runtime};

impl PullExecutor {
    pub async fn pull_agent<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_owner(
            app,
            client,
            http_url,
            sync_token,
            OwnerType::Agent,
            agent_id,
            write_queue,
        )
        .await
    }

    pub async fn pull_group<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_owner(
            app,
            client,
            http_url,
            sync_token,
            OwnerType::Group,
            group_id,
            write_queue,
        )
        .await
    }

    /// Compatibility facade for an old caller that only knows a topic id.
    /// The local database must resolve exactly one owner namespace before the
    /// request can cross the Wire 1.4 boundary.
    pub async fn pull_agent_topic<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let key = resolve_topic_key(app, "agent", topic_id).await?;
        pull_topic(app, client, http_url, sync_token, key, write_queue).await
    }

    /// Compatibility facade for an old caller that only knows a topic id.
    pub async fn pull_group_topic<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        let key = resolve_topic_key(app, "group", topic_id).await?;
        pull_topic(app, client, http_url, sync_token, key, write_queue).await
    }
}

async fn pull_owner<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    owner_type: OwnerType,
    owner_id: &str,
    write_queue: &DbWriteQueue,
) -> Result<(), String> {
    let item = json!({
        "entityType": "owner",
        "ownerType": owner_type.as_str(),
        "ownerId": owner_id,
    });
    PullExecutor::pull_entities_batch(app, client, http_url, sync_token, vec![item], write_queue)
        .await
}

async fn pull_topic<R: Runtime>(
    app: &AppHandle<R>,
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    key: TopicKey,
    write_queue: &DbWriteQueue,
) -> Result<(), String> {
    let item = json!({
        "entityType": "topic",
        "ownerType": key.owner_type,
        "ownerId": key.owner_id,
        "topicId": key.topic_id,
    });
    PullExecutor::pull_entities_batch(app, client, http_url, sync_token, vec![item], write_queue)
        .await
}

async fn resolve_topic_key<R: Runtime>(
    app: &AppHandle<R>,
    owner_type: &str,
    topic_id: &str,
) -> Result<TopicKey, String> {
    if !matches!(owner_type, "agent" | "group") || topic_id.is_empty() {
        return Err("topic pull requires a complete owner/topic identity".to_string());
    }
    let db = app.state::<DbState>();
    let rows = sqlx::query(
        "SELECT owner_id FROM topics
         WHERE owner_type = ? AND topic_id = ? AND deleted_at IS NULL
         ORDER BY owner_id",
    )
    .bind(owner_type)
    .bind(topic_id)
    .fetch_all(&db.pool)
    .await
    .map_err(|error| format!("topic identity lookup failed: {error}"))?;
    match rows.as_slice() {
        [row] => {
            let owner_id = row
                .try_get::<String, _>("owner_id")
                .map_err(|error| format!("topic owner identity decode failed: {error}"))?;
            Ok(TopicKey::new(owner_type, owner_id, topic_id))
        }
        [] => Err(format!(
            "topic {owner_type}/{topic_id} is missing or deleted"
        )),
        _ => Err(format!(
            "topic {owner_type}/{topic_id} is ambiguous; complete TopicKey is required"
        )),
    }
}
