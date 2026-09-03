use super::super::{DbWriteQueue, DbWriteTask, ExpectedMessageStates};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use rusqlite::Transaction;
use std::collections::HashSet;

pub(super) fn apply_task(
    tx: &Transaction<'_>,
    task: DbWriteTask,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    match task {
        DbWriteTask::Agent { id, dto } => apply_agent(tx, id, dto, owners),
        DbWriteTask::Group { id, dto } => apply_group(tx, id, dto, owners),
        DbWriteTask::Avatar {
            owner_type,
            owner_id,
            bytes,
        } => apply_avatar(tx, owner_type, owner_id, bytes),
        DbWriteTask::AgentTopic { topic_id, dto } => {
            apply_agent_topic(tx, topic_id, dto, owners, topics)
        }
        DbWriteTask::AgentTopicBatch { topics: batch } => {
            super::apply_agent_topics(tx, batch, owners, topics)
        }
        DbWriteTask::GroupTopic { topic_id, dto } => {
            apply_group_topic(tx, topic_id, dto, owners, topics)
        }
        DbWriteTask::GroupTopicBatch { topics: batch } => {
            super::apply_group_topics(tx, batch, owners, topics)
        }
        task => apply_remaining_task(tx, task, owners, topics),
    }
}

fn apply_remaining_task(
    tx: &Transaction<'_>,
    task: DbWriteTask,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    match task {
        DbWriteTask::TopicMessages {
            topic_id,
            messages,
            compressed_contents,
            render_bytes,
            content_hashes,
            skip_bubble,
        } => apply_topic_messages(
            tx,
            topic_id,
            messages,
            compressed_contents,
            render_bytes,
            content_hashes,
            skip_bubble,
            owners,
            topics,
        ),
        DbWriteTask::TopicMessagesCanonical {
            topic,
            messages,
            compressed_contents,
            render_bytes,
            expected_states,
            skip_bubble,
        } => apply_canonical_messages(
            tx,
            topic,
            messages,
            compressed_contents,
            render_bytes,
            expected_states,
            skip_bubble,
            owners,
            topics,
        ),
        DbWriteTask::DeleteTopic { topic, deleted_at } => {
            apply_delete_topic(tx, topic, deleted_at, owners)
        }
        DbWriteTask::DeleteMessage {
            message,
            deleted_at,
        } => apply_delete_message(tx, message, deleted_at, owners, topics),
        DbWriteTask::Flush { .. } => Err(rusqlite::Error::InvalidQuery),
        _ => unreachable!("non-message task was routed to apply_remaining_task"),
    }
}

fn apply_agent(
    tx: &Transaction<'_>,
    id: String,
    dto: crate::vcp_modules::sync_dto::AgentSyncDTO,
    owners: &mut HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    DbWriteQueue::rusqlite_upsert_agent(tx, &id, &dto)?;
    owners.insert(OwnerKey::new("agent", id));
    Ok(())
}

fn apply_group(
    tx: &Transaction<'_>,
    id: String,
    dto: crate::vcp_modules::sync_dto::GroupSyncDTO,
    owners: &mut HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    DbWriteQueue::rusqlite_upsert_group(tx, &id, &dto)?;
    owners.insert(OwnerKey::new("group", id));
    Ok(())
}

fn apply_avatar(
    tx: &Transaction<'_>,
    owner_type: String,
    owner_id: String,
    bytes: Vec<u8>,
) -> rusqlite::Result<()> {
    DbWriteQueue::rusqlite_upsert_avatar(tx, &owner_type, &owner_id, &bytes)
}

fn apply_agent_topic(
    tx: &Transaction<'_>,
    topic_id: String,
    dto: crate::vcp_modules::sync_dto::AgentTopicSyncDTO,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    let key = TopicKey::new("agent", dto.owner_id.clone(), topic_id);
    DbWriteQueue::rusqlite_upsert_agent_topic_for_key(tx, &key, &dto)?;
    record_topic(key, owners, topics);
    Ok(())
}

fn apply_group_topic(
    tx: &Transaction<'_>,
    topic_id: String,
    dto: crate::vcp_modules::sync_dto::GroupTopicSyncDTO,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    let key = TopicKey::new("group", dto.owner_id.clone(), topic_id);
    DbWriteQueue::rusqlite_upsert_group_topic_for_key(tx, &key, &dto)?;
    record_topic(key, owners, topics);
    Ok(())
}

fn record_topic(key: TopicKey, owners: &mut HashSet<OwnerKey>, topics: &mut HashSet<TopicKey>) {
    owners.insert(key.owner_key());
    topics.insert(key);
}

#[allow(clippy::too_many_arguments)]
fn apply_topic_messages(
    tx: &Transaction<'_>,
    topic_id: String,
    messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    compressed_contents: Vec<Vec<u8>>,
    render_bytes: Vec<Vec<u8>>,
    content_hashes: Vec<String>,
    skip_bubble: bool,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    let key = DbWriteQueue::rusqlite_resolve_topic_key(tx, &topic_id)?;
    if !skip_bubble {
        record_topic(key, owners, topics);
    }
    DbWriteQueue::rusqlite_upsert_messages_batch(
        tx,
        &topic_id,
        messages,
        compressed_contents,
        render_bytes,
        content_hashes,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_canonical_messages(
    tx: &Transaction<'_>,
    topic: TopicKey,
    messages: Vec<crate::vcp_modules::sync_dto::MessageSyncDTO>,
    compressed_contents: Vec<Vec<u8>>,
    render_bytes: Vec<Vec<u8>>,
    expected_states: Option<ExpectedMessageStates>,
    skip_bubble: bool,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    if !skip_bubble {
        record_topic(topic.clone(), owners, topics);
    }
    DbWriteQueue::rusqlite_upsert_messages_batch_for_key_if_unchanged(
        tx,
        &topic,
        messages,
        compressed_contents,
        render_bytes,
        expected_states.as_ref(),
    )
}

fn apply_delete_topic(
    tx: &Transaction<'_>,
    topic: TopicKey,
    deleted_at: i64,
    owners: &mut HashSet<OwnerKey>,
) -> rusqlite::Result<()> {
    if DbWriteQueue::rusqlite_delete_topic_for_key(tx, &topic, deleted_at)? {
        owners.insert(topic.owner_key());
    }
    Ok(())
}

fn apply_delete_message(
    tx: &Transaction<'_>,
    message: crate::vcp_modules::topic_types::MessageKey,
    deleted_at: i64,
    owners: &mut HashSet<OwnerKey>,
    topics: &mut HashSet<TopicKey>,
) -> rusqlite::Result<()> {
    if DbWriteQueue::rusqlite_delete_message_for_key(tx, &message, deleted_at)? {
        owners.insert(message.topic.owner_key());
        topics.insert(message.topic);
    }
    Ok(())
}
