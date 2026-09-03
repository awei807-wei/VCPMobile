use crate::vcp_modules::sync::sync_error::{parse_wire_sync_error, WireSyncError};
use crate::vcp_modules::sync::sync_types::validate_safe_non_negative_u64;
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_dto::MessageSyncDTO;
use crate::vcp_modules::sync_hash::HashAggregator;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct TopicNDJSONFrame {
    pub(crate) topic: TopicKey,
    pub(crate) messages: Vec<MessageSyncDTO>,
    pub(crate) error: Option<WireSyncError>,
    pub(crate) legacy_attachment_warnings: usize,
    pub(crate) warning_samples: Vec<String>,
}

/// Parse one Wire 1.4 `/messages/pull` NDJSON topic frame.
pub(crate) fn parse_topic_ndjson_frame(bytes: &[u8]) -> Result<TopicNDJSONFrame, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Malformed NDJSON topic frame: {error}"))?;
    let value = parse_strict_json(text)
        .map_err(|error| format!("Malformed NDJSON topic frame: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "NDJSON topic frame must be an object".to_string())?;
    validate_frame_fields(object)?;
    let topic = parse_topic_key(object)?;
    let ok = object
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("NDJSON frame for {} requires boolean ok", topic.topic_id))?;

    if ok {
        parse_success_frame(object, topic)
    } else {
        parse_failure_frame(object, topic)
    }
}

fn validate_frame_fields(object: &Map<String, Value>) -> Result<(), String> {
    for field in object.keys() {
        if !matches!(
            field.as_str(),
            "kind"
                | "topicId"
                | "ownerType"
                | "ownerId"
                | "ok"
                | "messages"
                | "error"
                | "legacyAttachmentWarnings"
                | "warningSamples"
        ) {
            return Err(format!("NDJSON topic frame contains unknown field {field}"));
        }
    }
    if object.get("kind").and_then(Value::as_str) != Some("topic") {
        return Err("NDJSON message pull frame kind must be topic".to_string());
    }
    for field in ["topicId", "ownerType", "ownerId", "ok"] {
        if !object.contains_key(field) {
            return Err(format!("NDJSON topic frame requires {field}"));
        }
    }
    Ok(())
}

fn parse_topic_key(object: &Map<String, Value>) -> Result<TopicKey, String> {
    let topic_id = non_empty_string(object.get("topicId"), "topicId")?;
    let owner_type = object
        .get("ownerType")
        .and_then(Value::as_str)
        .ok_or_else(|| "NDJSON topic frame ownerType must be a string".to_string())?;
    if !matches!(owner_type, "agent" | "group") {
        return Err(format!(
            "NDJSON topic frame has unsupported ownerType {owner_type}"
        ));
    }
    let owner_id = non_empty_string(object.get("ownerId"), "ownerId")?;
    Ok(TopicKey::new(owner_type, owner_id, topic_id))
}

fn parse_success_frame(
    object: &Map<String, Value>,
    topic: TopicKey,
) -> Result<TopicNDJSONFrame, String> {
    if object.contains_key("error") {
        return Err(format!(
            "successful NDJSON topic {} must not contain error",
            topic.topic_id
        ));
    }
    let raw_messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "NDJSON frame for {} requires messages array",
                topic.topic_id
            )
        })?;
    if raw_messages.len() > super::MAX_NDJSON_ENTITIES {
        return Err(format!(
            "NDJSON frame for {} exceeds {} message budget",
            topic.topic_id,
            super::MAX_NDJSON_ENTITIES
        ));
    }
    let has_warning_count = object.contains_key("legacyAttachmentWarnings");
    let has_warning_samples = object.contains_key("warningSamples");
    if has_warning_count != has_warning_samples {
        return Err(format!(
            "successful NDJSON topic {} requires warning count and samples together",
            topic.topic_id
        ));
    }
    let (legacy_attachment_warnings, warning_samples) = parse_warnings(object, &topic.topic_id)?;
    let mut messages = Vec::with_capacity(raw_messages.len());
    let mut ids = HashSet::with_capacity(raw_messages.len());
    for raw in raw_messages {
        let message = parse_message(raw, &topic)?;
        if !ids.insert(message.id.clone()) {
            return Err(format!(
                "Topic {} contains duplicate message {}",
                topic.topic_id, message.id
            ));
        }
        messages.push(message);
    }
    Ok(TopicNDJSONFrame {
        topic,
        messages,
        error: None,
        legacy_attachment_warnings,
        warning_samples,
    })
}

fn parse_failure_frame(
    object: &Map<String, Value>,
    topic: TopicKey,
) -> Result<TopicNDJSONFrame, String> {
    if object.contains_key("messages") {
        return Err(format!(
            "failed NDJSON topic {} must not contain messages",
            topic.topic_id
        ));
    }
    if object.contains_key("legacyAttachmentWarnings") || object.contains_key("warningSamples") {
        return Err(format!(
            "failed NDJSON topic {} must not contain warning fields",
            topic.topic_id
        ));
    }
    let raw_error = object
        .get("error")
        .ok_or_else(|| format!("failed NDJSON topic {} requires error", topic.topic_id))?;
    let error = parse_wire_sync_error(raw_error)
        .map_err(|error| format!("invalid NDJSON topic error for {}: {error}", topic.topic_id))?;
    Ok(TopicNDJSONFrame {
        topic,
        messages: Vec::new(),
        error: Some(error),
        legacy_attachment_warnings: 0,
        warning_samples: Vec::new(),
    })
}

fn parse_warnings(
    object: &Map<String, Value>,
    topic_id: &str,
) -> Result<(usize, Vec<String>), String> {
    let count = match object.get("legacyAttachmentWarnings") {
        None => 0,
        Some(Value::Number(value)) => {
            let value = value.as_u64().ok_or_else(|| {
                format!("NDJSON frame {topic_id} warning count must be non-negative")
            })?;
            usize::try_from(value).map_err(|_| {
                format!("NDJSON frame {topic_id} warning count exceeds platform limit")
            })?
        }
        Some(_) => {
            return Err(format!(
                "NDJSON frame {topic_id} warning count must be an integer"
            ))
        }
    };
    let samples = match object.get("warningSamples") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    format!("NDJSON frame {topic_id} warningSamples must contain strings")
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(format!(
                "NDJSON frame {topic_id} warningSamples must contain strings"
            ))
        }
    };
    if samples.len() > super::MAX_WARNING_SAMPLES {
        return Err(format!(
            "NDJSON frame {topic_id} warningSamples exceeds {} samples",
            super::MAX_WARNING_SAMPLES
        ));
    }
    if count == 0 && !samples.is_empty() {
        return Err(format!(
            "NDJSON frame {topic_id} warningSamples requires a positive warning count"
        ));
    }
    Ok((count, samples))
}

fn parse_message(value: &Value, topic: &TopicKey) -> Result<MessageSyncDTO, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("Topic {} contains a non-object message", topic.topic_id))?;
    let message_id = non_empty_string(object.get("id"), "message id")?;
    reject_tombstone_fields(object, &message_id)?;
    let message = serde_json::from_value::<MessageSyncDTO>(value.clone()).map_err(|error| {
        format!(
            "Message {}/{} violates the Wire 1.4 canonical DTO: {error}",
            topic.topic_id, message_id
        )
    })?;
    if message.id != message_id || message.role.is_empty() {
        return Err(format!(
            "Message {message_id} requires non-empty id and role"
        ));
    }
    if message
        .topic_id
        .as_deref()
        .is_some_and(|message_topic| message_topic != topic.topic_id)
    {
        return Err(format!(
            "Message {message_id} topicId conflicts with frame topic {}",
            topic.topic_id
        ));
    }
    validate_message_clocks(&message)?;
    validate_message_hash(&message)?;
    Ok(message)
}

fn reject_tombstone_fields(object: &Map<String, Value>, message_id: &str) -> Result<(), String> {
    let has_deleted_at = object.contains_key("deletedAt") || object.contains_key("deleted_at");
    let has_tombstone_marker = object.contains_key("tombstone")
        || object
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| status == "removed" || status == "deleted");
    if has_deleted_at || has_tombstone_marker {
        return Err(format!(
            "Tombstoned message {message_id} must not appear in a live pull frame"
        ));
    }
    Ok(())
}

fn validate_message_clocks(message: &MessageSyncDTO) -> Result<(), String> {
    validate_safe_non_negative_u64(
        message.timestamp,
        &format!("Message {} timestamp", message.id),
    )?;
    validate_safe_non_negative_u64(
        message.updated_at,
        &format!("Message {} updatedAt", message.id),
    )?;
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

fn validate_message_hash(message: &MessageSyncDTO) -> Result<(), String> {
    let Some(received) = message.content_hash.as_deref() else {
        return Ok(());
    };
    let expected = HashAggregator::compute_message_fingerprint_for_dto(message);
    if received != expected {
        return Err(format!(
            "Message {} contentHash does not match canonical content",
            message.id
        ));
    }
    Ok(())
}

fn non_empty_string(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{field} must be a non-empty string"))
}

pub(crate) fn validate_returned_topic_identity(
    frame: &TopicNDJSONFrame,
    expected: &HashSet<TopicKey>,
) -> Result<(), String> {
    if expected.contains(&frame.topic) {
        Ok(())
    } else {
        Err(format!(
            "NDJSON returned unexpected topic identity {}/{}/{}",
            frame.topic.owner_type, frame.topic.owner_id, frame.topic.topic_id
        ))
    }
}

pub(crate) fn validate_requested_message_ids(
    topic: &TopicKey,
    expected: Option<&HashSet<String>>,
    messages: &[MessageSyncDTO],
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = messages
        .iter()
        .map(|message| MessageKey::new(topic.clone(), message.id.clone()))
        .collect::<HashSet<_>>();
    let expected_keys = expected
        .iter()
        .map(|id| MessageKey::new(topic.clone(), id.clone()))
        .collect::<HashSet<_>>();
    if actual == expected_keys {
        return Ok(());
    }
    let mut missing = expected_keys
        .difference(&actual)
        .map(|key| key.msg_id.clone())
        .collect::<Vec<_>>();
    let mut unexpected = actual
        .difference(&expected_keys)
        .map(|key| key.msg_id.clone())
        .collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "NDJSON message set mismatch for {}/{}/{}: missing={missing:?}, unexpected={unexpected:?}",
        topic.owner_type, topic.owner_id, topic.topic_id
    ))
}

pub(crate) type ExpectedMessages = HashMap<TopicKey, Option<HashSet<String>>>;
