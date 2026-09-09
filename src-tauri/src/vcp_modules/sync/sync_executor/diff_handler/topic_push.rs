use super::action_dispatch::send_failure;
use super::context::DiffContext;
use super::phase;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::{AgentTopicSyncDTO, GroupTopicSyncDTO};
use crate::vcp_modules::sync_executor::PushExecutor;
use crate::vcp_modules::sync_types::{EntityPushData, EntityPushItem, OwnerType};
use crate::vcp_modules::topic_types::TopicKey;
use sqlx::Row;
use tauri::Manager;

#[derive(Debug, Clone)]
pub(crate) struct TopicPushRequest {
    pub(crate) key: TopicKey,
}

impl TopicPushRequest {
    pub(crate) fn new(key: TopicKey) -> Self {
        Self { key }
    }

    pub(crate) fn topic_id(&self) -> &str {
        &self.key.topic_id
    }
}

pub(crate) async fn spawn_topic_push(ctx: &DiffContext, requests: Vec<TopicPushRequest>) {
    let context = ctx.clone();
    ctx.task_tracker
        .spawn(async move {
            let db = context.app_handle.state::<DbState>();
            let batch = match build_topic_push_batch(&db.pool, &requests).await {
                Ok(batch) => batch,
                Err(error) => {
                    send_failure(&context, error.code, error.message, error.failed_topic_ids);
                    return;
                }
            };
            for chunk in batch.chunks(1000) {
                let sub_batch = chunk.to_vec();
                let count = sub_batch.len() as u32;
                let failed_topic_ids = sub_batch
                    .iter()
                    .filter_map(|item| match item {
                        EntityPushItem::Topic { topic_id, .. } => Some(topic_id.clone()),
                        EntityPushItem::Owner { .. } => None,
                    })
                    .take(8)
                    .collect();
                if let Err(error) = PushExecutor::push_entities_batch(
                    &context.app_handle,
                    &context.http_client,
                    &context.base_url,
                    &context.token,
                    sub_batch,
                )
                .await
                {
                    send_failure(
                        &context,
                        "TOPIC_PUSH_FAILED",
                        format!("Batch topic push failed: {error}"),
                        failed_topic_ids,
                    );
                    return;
                }
                phase::complete_operations(&context, count);
            }
        })
        .await;
}

struct TopicPushFailure {
    code: String,
    message: String,
    failed_topic_ids: Vec<String>,
}

impl TopicPushFailure {
    fn one(code: &str, message: impl Into<String>, topic_id: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            failed_topic_ids: vec![topic_id.to_string()],
        }
    }
}

async fn build_topic_push_batch(
    pool: &sqlx::SqlitePool,
    requests: &[TopicPushRequest],
) -> Result<Vec<EntityPushItem>, TopicPushFailure> {
    let mut batch = Vec::with_capacity(requests.len());
    for request in requests {
        let row = sqlx::query(
            "SELECT topic_id, title, created_at, locked, unread, owner_id, owner_type
             FROM topics
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ? AND deleted_at IS NULL",
        )
        .bind(&request.key.owner_type)
        .bind(&request.key.owner_id)
        .bind(&request.key.topic_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| {
            TopicPushFailure::one(
                "TOPIC_PUSH_DB_FAILED",
                format!(
                    "Failed to load topic {}/{}/{} for push: {error}",
                    request.key.owner_type, request.key.owner_id, request.key.topic_id
                ),
                request.topic_id(),
            )
        })?
        .ok_or_else(|| {
            TopicPushFailure::one(
                "TOPIC_PUSH_SOURCE_MISSING",
                format!(
                    "Topic selected for push is missing: {}/{}/{}",
                    request.key.owner_type, request.key.owner_id, request.key.topic_id
                ),
                request.topic_id(),
            )
        })?;
        let item = topic_push_item(&row, &request.key).map_err(|message| {
            TopicPushFailure::one("TOPIC_PUSH_DB_DECODE_FAILED", message, request.topic_id())
        })?;
        batch.push(item);
    }
    Ok(batch)
}

fn topic_push_item(
    row: &sqlx::sqlite::SqliteRow,
    key: &TopicKey,
) -> Result<EntityPushItem, String> {
    let topic_id: String = row
        .try_get("topic_id")
        .map_err(|error| format!("topic id: {error}"))?;
    let title: String = row
        .try_get("title")
        .map_err(|error| format!("title: {error}"))?;
    let created_at: i64 = row
        .try_get("created_at")
        .map_err(|error| format!("created_at: {error}"))?;
    let locked: i64 = row
        .try_get("locked")
        .map_err(|error| format!("locked: {error}"))?;
    let unread: i64 = row
        .try_get("unread")
        .map_err(|error| format!("unread: {error}"))?;
    let owner_id: String = row
        .try_get("owner_id")
        .map_err(|error| format!("owner_id: {error}"))?;
    let owner_type: String = row
        .try_get("owner_type")
        .map_err(|error| format!("owner_type: {error}"))?;
    validate_topic_owner(key, &owner_type, &owner_id, &topic_id)?;
    let owner_type = OwnerType::try_from(owner_type.as_str())
        .map_err(|_| format!("Topic {key:?} has an invalid owner type"))?;
    let data = match owner_type {
        OwnerType::Agent => EntityPushData::AgentTopic(AgentTopicSyncDTO {
            id: topic_id.clone(),
            name: title,
            created_at,
            locked: locked != 0,
            unread: unread != 0,
            owner_id: owner_id.clone(),
        }),
        OwnerType::Group => EntityPushData::GroupTopic(GroupTopicSyncDTO {
            id: topic_id.clone(),
            name: title,
            created_at,
            owner_id: owner_id.clone(),
        }),
    };
    let item = EntityPushItem::Topic {
        owner_type,
        owner_id,
        topic_id,
        data,
    };
    if !item.is_consistent() {
        return Err(format!("Topic {key:?} push identity is inconsistent"));
    }
    Ok(item)
}

pub(crate) fn validate_topic_owner(
    key: &TopicKey,
    actual_type: &str,
    actual_id: &str,
    actual_topic_id: &str,
) -> Result<(), String> {
    if !key.is_valid()
        || !matches!(actual_type, "agent" | "group")
        || actual_id.is_empty()
        || actual_topic_id.is_empty()
        || actual_id != key.owner_id
        || actual_type != key.owner_type
        || actual_topic_id != key.topic_id
    {
        return Err(format!(
            "Topic {}/{}/{} owner does not match the manifest decision",
            key.owner_type, key.owner_id, key.topic_id
        ));
    }
    Ok(())
}
