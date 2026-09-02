use super::ndjson_codec::{http_status_error, read_response_limited};
use super::PullExecutor;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use serde_json::Value;
use tauri::{AppHandle, Runtime};

const DIRECT_RESPONSE_LIMIT: usize = 10 * 1024 * 1024;

enum DirectEntityKind {
    Agent,
    Group,
    AgentTopic,
    GroupTopic,
}

impl DirectEntityKind {
    fn wire_name(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Group => "group",
            Self::AgentTopic => "agent_topic",
            Self::GroupTopic => "group_topic",
        }
    }
}

impl PullExecutor {
    pub async fn pull_agent<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        agent_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_direct_entity(
            client,
            http_url,
            sync_token,
            agent_id,
            write_queue,
            DirectEntityKind::Agent,
        )
        .await
    }

    pub async fn pull_group<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        group_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_direct_entity(
            client,
            http_url,
            sync_token,
            group_id,
            write_queue,
            DirectEntityKind::Group,
        )
        .await
    }

    pub async fn pull_agent_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_direct_entity(
            client,
            http_url,
            sync_token,
            topic_id,
            write_queue,
            DirectEntityKind::AgentTopic,
        )
        .await
    }

    pub async fn pull_group_topic<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        topic_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        pull_direct_entity(
            client,
            http_url,
            sync_token,
            topic_id,
            write_queue,
            DirectEntityKind::GroupTopic,
        )
        .await
    }
}

async fn pull_direct_entity(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    id: &str,
    write_queue: &DbWriteQueue,
    kind: DirectEntityKind,
) -> Result<(), String> {
    let entity_type = kind.wire_name();
    let url = format!("{http_url}/api/mobile-sync/download-entity?id={id}&type={entity_type}");
    let response = client
        .get(&url)
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let (status, bytes) = read_response_limited(
        response,
        DIRECT_RESPONSE_LIMIT,
        &format!("Pull {entity_type}"),
    )
    .await?;
    if !status.is_success() {
        return Err(http_status_error(
            &format!("Pull {entity_type}"),
            status,
            &bytes,
        ));
    }
    let value = parse_strict_body(&bytes, &format!("Pull {entity_type}"))?;
    submit_direct_entity(write_queue, id, kind, value).await
}

fn parse_strict_body(bytes: &[u8], operation: &str) -> Result<Value, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{operation} returned invalid UTF-8: {error}"))?;
    parse_strict_json(text).map_err(|error| format!("{operation} returned invalid JSON: {error}"))
}

async fn submit_direct_entity(
    write_queue: &DbWriteQueue,
    id: &str,
    kind: DirectEntityKind,
    value: Value,
) -> Result<(), String> {
    match kind {
        DirectEntityKind::Agent => {
            let dto = serde_json::from_value::<AgentSyncDTO>(value)
                .map_err(|error| format!("Pull agent returned invalid JSON: {error}"))?;
            write_queue
                .submit(DbWriteTask::Agent {
                    id: id.to_string(),
                    dto,
                })
                .await
        }
        DirectEntityKind::Group => {
            let dto = serde_json::from_value::<GroupSyncDTO>(value)
                .map_err(|error| format!("Pull group returned invalid JSON: {error}"))?;
            write_queue
                .submit(DbWriteTask::Group {
                    id: id.to_string(),
                    dto,
                })
                .await
        }
        DirectEntityKind::AgentTopic => {
            let dto = serde_json::from_value::<AgentTopicSyncDTO>(value)
                .map_err(|error| format!("Pull agent topic returned invalid JSON: {error}"))?;
            write_queue
                .submit(DbWriteTask::AgentTopic {
                    topic_id: id.to_string(),
                    dto,
                })
                .await
        }
        DirectEntityKind::GroupTopic => {
            let dto = serde_json::from_value::<GroupTopicSyncDTO>(value)
                .map_err(|error| format!("Pull group topic returned invalid JSON: {error}"))?;
            write_queue
                .submit(DbWriteTask::GroupTopic {
                    topic_id: id.to_string(),
                    dto,
                })
                .await
        }
    }
}
