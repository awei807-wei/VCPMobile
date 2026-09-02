use super::context::DiffContext;
use super::diff_item_validation::ValidatedDiffItem;
use crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor;
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::SyncDataType;

pub(crate) async fn execute_delete(
    ctx: &DiffContext,
    item: &ValidatedDiffItem,
) -> Result<(), String> {
    let deleted_at = item
        .deleted_at
        .ok_or_else(|| format!("DELETE action for {} is missing deletedAt", item.id))?;
    match &ctx.data_type {
        SyncDataType::Agent => {
            DeleteExecutor::soft_delete_agent(&ctx.app_handle, &item.id, deleted_at).await
        }
        SyncDataType::Group => {
            DeleteExecutor::soft_delete_group(&ctx.app_handle, &item.id, deleted_at).await
        }
        SyncDataType::Avatar => execute_avatar_delete(ctx, &item.id, deleted_at).await,
        SyncDataType::Topic => {
            DeleteExecutor::soft_delete_topic(&ctx.app_handle, &item.id, deleted_at).await
        }
        SyncDataType::Message => {
            let topic_id = item
                .topic_id
                .as_deref()
                .ok_or_else(|| format!("Message delete for {} is missing topicId", item.id))?;
            DeleteExecutor::soft_delete_message(&ctx.app_handle, topic_id, &item.id, deleted_at)
                .await
        }
    }
}

async fn execute_avatar_delete(ctx: &DiffContext, id: &str, deleted_at: i64) -> Result<(), String> {
    let mut parts = id.splitn(2, ':');
    let owner_type = parts.next().unwrap_or_default();
    let owner_id = parts.next().unwrap_or_default();
    if !crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) {
        return Err(format!("invalid avatar owner identity {id}"));
    }
    DeleteExecutor::soft_delete_avatar(&ctx.app_handle, owner_type, owner_id, deleted_at).await
}

pub(crate) fn notify_push_delete(
    ctx: &DiffContext,
    item: &ValidatedDiffItem,
) -> Result<(), String> {
    let deleted_at = item
        .deleted_at
        .ok_or_else(|| format!("PUSH_DELETE action for {} is missing deletedAt", item.id))?;
    let command = if ctx.data_type == SyncDataType::Message {
        SyncCommand::NotifyMessageDelete {
            topic_id: item
                .topic_id
                .clone()
                .ok_or_else(|| format!("Message delete for {} is missing topicId", item.id))?,
            message_id: item.id.clone(),
            deleted_at,
        }
    } else {
        SyncCommand::NotifyDelete {
            data_type: ctx.data_type.clone(),
            id: item.id.clone(),
            deleted_at,
        }
    };
    ctx.tx_internal
        .send(command)
        .map_err(|error| error.to_string())
}
