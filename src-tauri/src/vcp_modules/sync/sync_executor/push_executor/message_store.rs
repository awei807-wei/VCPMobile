use super::message_attachments::load_message_attachments;
use super::types::{
    canonical_sha256, BoundedJsonLine, OutboundMessageSyncDTO, MAX_NDJSON_LINE_BYTES,
    MESSAGE_PAGE_SIZE,
};
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::sync_dto::{
    AgentMessageSyncDTO, AttachmentSyncDTO, GroupMessageSyncDTO, UserMessageSyncDTO,
};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashMap;
use std::io::Write;
use tauri::{AppHandle, Manager, Runtime};

pub(super) async fn query_avatar_color(
    pool: &sqlx::SqlitePool,
    agent_id: &str,
) -> Result<Option<String>, String> {
    if agent_id.is_empty() {
        return Ok(None);
    }
    sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "SELECT dominant_color FROM avatars WHERE owner_id = ? AND owner_type = 'agent' AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(pool)
    .await
    .map(|color| color.flatten())
    .map_err(|error| format!("Avatar color query failed for {agent_id}: {error}"))
}

pub(super) async fn load_outbound_message_page(
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    cursor: Option<(i64, &str)>,
) -> Result<Vec<crate::vcp_modules::chat_manager::ChatMessage>, String> {
    let mut query = if cursor.is_some() {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash
             FROM messages
             WHERE topic_id = ? AND deleted_at IS NULL
               AND (timestamp > ? OR (timestamp = ? AND msg_id > ?))
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    } else {
        sqlx::query(
            "SELECT msg_id, role, name, agent_id, content, timestamp, is_group_message,
                    group_id, finish_reason, content_hash
             FROM messages
             WHERE topic_id = ? AND deleted_at IS NULL
             ORDER BY timestamp ASC, msg_id ASC
             LIMIT ?",
        )
    };
    query = query.bind(topic_id);
    if let Some((timestamp, message_id)) = cursor {
        query = query.bind(timestamp).bind(timestamp).bind(message_id);
    }
    query = query.bind(MESSAGE_PAGE_SIZE as i64);
    let rows = query
        .fetch_all(&mut **tx)
        .await
        .map_err(|error| format!("Message page query failed for {topic_id}: {error}"))?;

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        messages.push(decode_message_row(&row, topic_id)?);
    }
    load_message_attachments(tx, topic_id, &mut messages).await
}

fn decode_message_row(
    row: &sqlx::sqlite::SqliteRow,
    topic_id: &str,
) -> Result<crate::vcp_modules::chat_manager::ChatMessage, String> {
    let message_id: String = row
        .try_get("msg_id")
        .map_err(|error| format!("Message id decode failed for {topic_id}: {error}"))?;
    let timestamp: i64 = row.try_get("timestamp").map_err(|error| {
        format!("Message timestamp decode failed for {topic_id}/{message_id}: {error}")
    })?;
    let timestamp = u64::try_from(timestamp)
        .map_err(|_| format!("Message {topic_id}/{message_id} has a negative timestamp"))?;
    let content_hash: String = row.try_get("content_hash").map_err(|error| {
        format!("Message hash decode failed for {topic_id}/{message_id}: {error}")
    })?;
    let is_group_message: i64 = row.try_get("is_group_message").map_err(|error| {
        format!("Message group flag decode failed for {topic_id}/{message_id}: {error}")
    })?;
    let content = crate::vcp_modules::persistence::message_content_storage::decode_message_content(
        row, "content",
    )
    .map_err(|error| format!("Message content decode failed for {topic_id}: {error}"))?;
    Ok(crate::vcp_modules::chat_manager::ChatMessage {
        id: message_id,
        role: row
            .try_get("role")
            .map_err(|error| format!("Message role decode failed for {topic_id}: {error}"))?,
        name: row
            .try_get("name")
            .map_err(|error| format!("Message name decode failed for {topic_id}: {error}"))?,
        content,
        timestamp,
        is_thinking: Some(false),
        agent_id: row
            .try_get("agent_id")
            .map_err(|error| format!("Message agent decode failed for {topic_id}: {error}"))?,
        group_id: row
            .try_get("group_id")
            .map_err(|error| format!("Message group decode failed for {topic_id}: {error}"))?,
        topic_id: Some(topic_id.to_string()),
        is_group_message: Some(is_group_message != 0),
        finish_reason: row.try_get("finish_reason").map_err(|error| {
            format!("Message finish reason decode failed for {topic_id}: {error}")
        })?,
        attachments: None,
        blocks: None,
        shell: None,
        content_hash: (!content_hash.is_empty()).then_some(content_hash),
    })
}

async fn build_message_dto<R: Runtime>(
    app: &AppHandle<R>,
    message: crate::vcp_modules::chat_manager::ChatMessage,
    owner_type: &str,
    avatar_colors: &mut HashMap<String, String>,
) -> Result<OutboundMessageSyncDTO, String> {
    if message.id.is_empty() || message.role.is_empty() {
        return Err("Outbound messages require non-empty id and role".to_string());
    }
    if message.role == "user" {
        return build_user_dto(message);
    }
    if !matches!(owner_type, "agent" | "group") {
        return Err(format!(
            "Outbound message owner type {owner_type} is unsupported"
        ));
    }
    let agent_id = message.agent_id.unwrap_or_default();
    let avatar_color = if let Some(color) = avatar_colors.get(&agent_id) {
        color.clone()
    } else {
        let color = query_avatar_color(&app.state::<DbState>().pool, &agent_id)
            .await?
            .unwrap_or_else(|| "#6B7280".to_string());
        avatar_colors.insert(agent_id.clone(), color.clone());
        color
    };
    if owner_type == "group" {
        Ok(OutboundMessageSyncDTO::Group(GroupMessageSyncDTO {
            id: message.id,
            role: message.role,
            name: message.name,
            content: message.content,
            timestamp: message.timestamp,
            agent_id,
            group_id: message.group_id.unwrap_or_default(),
            topic_id: message.topic_id.unwrap_or_default(),
            is_group_message: true,
            avatar_color,
            content_hash: message.content_hash,
        }))
    } else {
        Ok(OutboundMessageSyncDTO::Agent(AgentMessageSyncDTO {
            id: message.id,
            role: message.role,
            name: message.name,
            content: message.content,
            timestamp: message.timestamp,
            agent_id,
            is_thinking: message.is_thinking,
            finish_reason: message.finish_reason,
            avatar_color,
            content_hash: message.content_hash,
        }))
    }
}

fn build_user_dto(
    message: crate::vcp_modules::chat_manager::ChatMessage,
) -> Result<OutboundMessageSyncDTO, String> {
    let attachments = message
        .attachments
        .map(|items| {
            items
                .into_iter()
                .map(|attachment| {
                    let hash = attachment
                        .hash
                        .as_deref()
                        .and_then(canonical_sha256)
                        .ok_or_else(|| {
                            format!(
                                "Outbound message {} attachment {} has no valid SHA-256 hash",
                                message.id, attachment.name
                            )
                        })?;
                    Ok(AttachmentSyncDTO {
                        r#type: attachment.r#type,
                        name: attachment.name,
                        size: attachment.size,
                        hash,
                        status: attachment.status,
                        extracted_text: attachment.extracted_text,
                        image_frames: attachment.image_frames,
                        created_at: attachment.created_at,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;
    Ok(OutboundMessageSyncDTO::User(UserMessageSyncDTO {
        id: message.id,
        role: message.role,
        name: message.name,
        content: message.content,
        timestamp: message.timestamp,
        attachments,
        content_hash: message.content_hash,
    }))
}

pub(super) async fn serialize_topic_messages<R: Runtime>(
    app: &AppHandle<R>,
    tx: &mut Transaction<'_, Sqlite>,
    topic_id: &str,
    owner_type: &str,
    owner_id: &str,
    expected_message_count: usize,
) -> Result<Vec<u8>, String> {
    let mut line = BoundedJsonLine::new(MAX_NDJSON_LINE_BYTES);
    write_json_prefix(&mut line, topic_id, owner_type, owner_id)?;
    let mut cursor: Option<(i64, String)> = None;
    let mut serialized_count = 0usize;
    let mut avatar_colors = HashMap::new();
    loop {
        let page = load_outbound_message_page(
            tx,
            topic_id,
            cursor
                .as_ref()
                .map(|(timestamp, id)| (*timestamp, id.as_str())),
        )
        .await?;
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for message in page {
            let timestamp = i64::try_from(message.timestamp).map_err(|_| {
                format!(
                    "Outbound message {} timestamp exceeds the supported range",
                    message.id
                )
            })?;
            let next_cursor = (timestamp, message.id.clone());
            if serialized_count > 0 {
                line.write_all(b",").map_err(|error| {
                    format!("Message push separator failed for {topic_id}: {error}")
                })?;
            }
            let dto = build_message_dto(app, message, owner_type, &mut avatar_colors).await?;
            serde_json::to_writer(&mut line, &dto).map_err(|error| format!("Message push topic {topic_id} exceeds the 32 MiB line limit or contains invalid data: {error}"))?;
            serialized_count = serialized_count
                .checked_add(1)
                .ok_or_else(|| "Message push count overflow".to_string())?;
            cursor = Some(next_cursor);
        }
        if page_len < MESSAGE_PAGE_SIZE {
            break;
        }
    }
    if serialized_count != expected_message_count {
        return Err(format!("Message push topic {topic_id} changed during serialization: expected {expected_message_count}, got {serialized_count}"));
    }
    line.write_all(b"]}\n")
        .map_err(|error| format!("Message push suffix failed for {topic_id}: {error}"))?;
    Ok(line.into_bytes())
}

fn write_json_prefix(
    line: &mut BoundedJsonLine,
    topic_id: &str,
    owner_type: &str,
    owner_id: &str,
) -> Result<(), String> {
    line.write_all(br#"{"topicId":"#)
        .map_err(|error| format!("Message push prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut *line, topic_id)
        .map_err(|error| format!("Message push topic id serialization failed: {error}"))?;
    line.write_all(b",\"ownerType\":")
        .map_err(|error| format!("Message push owner prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut *line, owner_type)
        .map_err(|error| format!("Message push owner type serialization failed: {error}"))?;
    line.write_all(b",\"ownerId\":")
        .map_err(|error| format!("Message push owner prefix failed for {topic_id}: {error}"))?;
    serde_json::to_writer(&mut *line, owner_id)
        .map_err(|error| format!("Message push owner id serialization failed: {error}"))?;
    line.write_all(b",\"messages\":[")
        .map_err(|error| format!("Message push prefix failed for {topic_id}: {error}"))?;
    Ok(())
}
