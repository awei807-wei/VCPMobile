use super::MAX_NDJSON_ENTITIES;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::{
    DbWriteQueue, DbWriteTask, ExpectedMessageStates, SNAPSHOT_STALE_MARKER,
};
use crate::vcp_modules::message_repository::{ContentCompressor, MessageRenderCompiler};
use crate::vcp_modules::sync::sync_types::validate_safe_non_negative_u64;
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::HashSet;
use tauri::{Manager, Runtime};

type PreparedMessages = (Vec<MessageSyncDTO>, Vec<Vec<u8>>, Vec<Vec<u8>>);

struct AttachmentBinding {
    message_id: String,
    hash: String,
    order: i32,
}

/// Validate and persist one canonical topic batch without materializing local
/// filesystem paths from wire data. The queue owns the SQLite transaction and
/// keeps compressed content, attachment relations and optional render cache in
/// one atomic write.
pub(crate) async fn process_topic_messages<R: Runtime>(
    app: &tauri::AppHandle<R>,
    topic: &TopicKey,
    messages: Vec<MessageSyncDTO>,
    expected_states: Option<ExpectedMessageStates>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
) -> Result<(usize, usize), String> {
    validate_messages(topic, &messages)?;
    let parsed_count = messages.len();
    if parsed_count == 0 {
        return Ok((0, 0));
    }
    let attachment_bindings = collect_attachment_bindings(&messages)?;
    let prepared = prepare_messages(messages, prerender_enabled).await?;
    write_queue
        .submit(DbWriteTask::TopicMessagesCanonical {
            topic: topic.clone(),
            messages: prepared.0,
            compressed_contents: prepared.1,
            render_bytes: prepared.2,
            expected_states,
            skip_bubble: false,
        })
        .await?;
    // A pull topic is the transaction boundary. The explicit flush also
    // propagates queue/storage errors to this topic's result before the next
    // NDJSON frame is consumed.
    write_queue
        .flush()
        .await
        .map_err(|error| map_write_error(topic, error))?;
    reconcile_attachment_bindings(app, topic, &attachment_bindings).await?;
    Ok((parsed_count, 0))
}

fn map_write_error(topic: &TopicKey, error: String) -> String {
    if !error.contains(SNAPSHOT_STALE_MARKER) {
        return error;
    }
    encode_local_sync_error(
        SNAPSHOT_STALE_MARKER,
        SyncErrorStage::Messages,
        "Local message changed after the Phase 3 snapshot; restarting sync",
        vec![topic.topic_id.clone()],
    )
}

fn collect_attachment_bindings(
    messages: &[MessageSyncDTO],
) -> Result<Vec<AttachmentBinding>, String> {
    let mut bindings = Vec::new();
    for message in messages {
        for (index, attachment) in message
            .attachments
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let order = attachment.attachment_order.unwrap_or(index as i32);
            if order < 0 {
                return Err(format!(
                    "Message {} attachment {} has a negative attachment order",
                    message.id, attachment.name
                ));
            }
            bindings.push(AttachmentBinding {
                message_id: message.id.clone(),
                hash: attachment.hash.clone(),
                order,
            });
        }
    }
    Ok(bindings)
}

async fn reconcile_attachment_bindings<R: Runtime>(
    app: &tauri::AppHandle<R>,
    topic: &TopicKey,
    bindings: &[AttachmentBinding],
) -> Result<(), String> {
    if bindings.is_empty() {
        return Ok(());
    }
    let db = app.state::<DbState>();
    let pool = &db.pool;
    for binding in bindings {
        let resolved =
            crate::vcp_modules::file_manager::resolve_attachment_cas_file(app, pool, &binding.hash)
                .await;
        let (src, status) = match resolved {
            Ok(file) => (format!("file://{}", file.path.to_string_lossy()), "ready"),
            Err(_) => {
                sqlx::query("UPDATE attachments SET internal_path = '' WHERE hash = ?")
                    .bind(&binding.hash)
                    .execute(pool)
                    .await
                    .map_err(|error| format!("attachment CAS unlink failed: {error}"))?;
                (String::new(), "desktop_only")
            }
        };
        let updated = sqlx::query(
            "UPDATE message_attachments SET src = ?, status = ?
             WHERE owner_type = ? AND owner_id = ? AND topic_id = ?
               AND msg_id = ? AND hash = ? AND attachment_order = ?
               AND deleted_at IS NULL",
        )
        .bind(src)
        .bind(status)
        .bind(&topic.owner_type)
        .bind(&topic.owner_id)
        .bind(&topic.topic_id)
        .bind(&binding.message_id)
        .bind(&binding.hash)
        .bind(binding.order)
        .execute(pool)
        .await
        .map_err(|error| format!("attachment binding update failed: {error}"))?;
        if updated.rows_affected() != 1 {
            return Err(format!(
                "attachment binding disappeared for message {}",
                binding.message_id
            ));
        }
    }
    Ok(())
}

fn validate_messages(topic: &TopicKey, messages: &[MessageSyncDTO]) -> Result<(), String> {
    if !topic.is_valid() {
        return Err("message pull requires a complete TopicKey".to_string());
    }
    if messages.len() > MAX_NDJSON_ENTITIES {
        return Err(format!(
            "Topic {}/{}/{} exceeds the message budget",
            topic.owner_type, topic.owner_id, topic.topic_id
        ));
    }
    let mut ids = HashSet::with_capacity(messages.len());
    for message in messages {
        validate_message(topic, message, &mut ids)?;
    }
    Ok(())
}

fn validate_message(
    topic: &TopicKey,
    message: &MessageSyncDTO,
    ids: &mut HashSet<String>,
) -> Result<(), String> {
    if message.id.is_empty() || message.role.is_empty() {
        return Err("canonical message requires non-empty id and role".to_string());
    }
    if !ids.insert(message.id.clone()) {
        return Err(format!(
            "Topic {} contains duplicate message {}",
            topic.topic_id, message.id
        ));
    }
    if message
        .topic_id
        .as_deref()
        .is_some_and(|message_topic| message_topic != topic.topic_id)
    {
        return Err(format!(
            "Message {} topicId conflicts with {}",
            message.id, topic.topic_id
        ));
    }
    validate_safe_non_negative_u64(
        message.timestamp,
        &format!("Message {} timestamp", message.id),
    )?;
    validate_safe_non_negative_u64(
        message.updated_at,
        &format!("Message {} updatedAt", message.id),
    )?;
    validate_content_hash(message)?;
    validate_message_attachments(message)
}

fn validate_content_hash(message: &MessageSyncDTO) -> Result<(), String> {
    if let Some(received_hash) = message.content_hash.as_deref() {
        let expected_hash = HashAggregator::compute_message_fingerprint_for_dto(message);
        if received_hash != expected_hash {
            return Err(format!(
                "Message {} contentHash does not match canonical content",
                message.id
            ));
        }
    }
    Ok(())
}

fn validate_message_attachments(message: &MessageSyncDTO) -> Result<(), String> {
    if let Some(attachments) = &message.attachments {
        for attachment in attachments {
            validate_safe_non_negative_u64(
                attachment.size,
                &format!("Message {} attachment {} size", message.id, attachment.name),
            )?;
            if let Some(created_at) = attachment.created_at {
                validate_safe_non_negative_u64(
                    created_at,
                    &format!(
                        "Message {} attachment {} createdAt",
                        message.id, attachment.name
                    ),
                )?;
            }
        }
    }
    Ok(())
}

async fn prepare_messages(
    messages: Vec<MessageSyncDTO>,
    prerender_enabled: bool,
) -> Result<PreparedMessages, String> {
    tokio::task::spawn_blocking(move || {
        let mut compressed = Vec::with_capacity(messages.len());
        let mut renders = Vec::with_capacity(messages.len());
        for message in &messages {
            compressed.push(ContentCompressor::compress(&message.content)?);
            renders.push(compile_render(message, prerender_enabled));
        }
        Ok((messages, compressed, renders))
    })
    .await
    .map_err(|error| format!("message preparation task failed: {error}"))?
}

fn compile_render(message: &MessageSyncDTO, enabled: bool) -> Vec<u8> {
    if !enabled {
        return Vec::new();
    }
    let content = message.content.clone();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let blocks = MessageRenderCompiler::compile(&content);
        MessageRenderCompiler::serialize(&blocks).unwrap_or_default()
    })) {
        Ok(bytes) => bytes,
        Err(_) => {
            log::warn!(
                "[PullExecutor] pre-render panicked for canonical message {}",
                message.id
            );
            Vec::new()
        }
    }
}
