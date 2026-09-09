use super::context::DiffContext;
use super::manifest::ManifestDecision;
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::ManifestAction;

/// Apply a remote manifest tombstone to local storage.
///
/// Every delete is dispatched with its complete Wire 1.4 identity. In
/// particular, topics must never be reduced to a bare `topicId`, because that
/// identifier is only unique inside an owner namespace.
pub(crate) async fn execute_delete(
    ctx: &DiffContext,
    item: &ManifestDecision,
) -> Result<(), String> {
    let deleted_at = item.deleted_at().ok_or_else(|| {
        format!(
            "{} action for {} is missing deletedAt",
            item.action(),
            item.display_id()
        )
    })?;
    match item {
        ManifestDecision::Owner(owner) => match owner.owner_type {
            crate::vcp_modules::sync_types::OwnerType::Agent => {
                crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor::soft_delete_agent(
                    &ctx.app_handle,
                    &owner.owner_id,
                    deleted_at,
                )
                .await
            }
            crate::vcp_modules::sync_types::OwnerType::Group => {
                crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor::soft_delete_group(
                    &ctx.app_handle,
                    &owner.owner_id,
                    deleted_at,
                )
                .await
            }
        },
        ManifestDecision::Avatar(avatar) => {
            crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor::soft_delete_avatar(
                &ctx.app_handle,
                avatar.owner_type.as_str(),
                &avatar.owner_id,
                deleted_at,
            )
            .await
        }
        ManifestDecision::Topic(topic) => {
            let key = crate::vcp_modules::topic_types::TopicKey::new(
                topic.owner_type.as_str(),
                &topic.owner_id,
                &topic.topic_id,
            );
            crate::vcp_modules::sync_executor::delete_executor::DeleteExecutor::soft_delete_topic(
                &ctx.app_handle,
                &key,
                deleted_at,
            )
            .await
        }
    }
}

/// Notify the desktop of a local tombstone using the exact Wire 1.4 frame.
pub(crate) fn notify_push_delete(ctx: &DiffContext, item: &ManifestDecision) -> Result<(), String> {
    if !matches!(item.action(), ManifestAction::PushDelete) {
        return Err(format!(
            "delete notification requires PUSH_DELETE, got {}",
            item.action()
        ));
    }
    let deleted_at = item.deleted_at().ok_or_else(|| {
        format!(
            "PUSH_DELETE action for {} is missing deletedAt",
            item.display_id()
        )
    })?;
    ctx.tx_internal
        .send(SyncCommand::NotifyDelete {
            target: item.delete_target(),
            deleted_at,
        })
        .map_err(|error| error.to_string())
}
