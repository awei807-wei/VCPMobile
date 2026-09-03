mod avatar;
mod entities;
mod http;
mod message_attachments;
mod message_store;
mod ndjson;
mod orchestration;
mod tombstone;
mod types;

pub use types::PushBatchResult;

use crate::vcp_modules::sync_types::EntityPushItem;
use crate::vcp_modules::topic_types::TopicKey;
use tauri::{AppHandle, Runtime};

pub struct PushExecutor;

impl PushExecutor {
    pub async fn push_agent<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
    ) -> Result<(), String> {
        entities::push_agent(app, client, http_url, sync_token, agent_id).await
    }

    pub async fn push_group<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
    ) -> Result<(), String> {
        entities::push_group(app, client, http_url, sync_token, group_id).await
    }

    pub async fn push_entities_batch<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        items: Vec<EntityPushItem>,
    ) -> Result<(), String> {
        entities::push_entities_batch(client, http_url, sync_token, items).await
    }

    pub async fn push_avatar<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
    ) -> Result<(), String> {
        avatar::push_avatar(app, client, http_url, sync_token, owner_type, owner_id).await
    }

    pub async fn push_messages_batch<R: Runtime>(
        app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topics: &[TopicKey],
    ) -> Result<Vec<PushBatchResult>, String> {
        orchestration::push_messages_batch(app, client, http_url, sync_token, topics).await
    }
}

#[cfg(test)]
#[path = "push_executor_contract_tests.rs"]
mod contract_tests;
