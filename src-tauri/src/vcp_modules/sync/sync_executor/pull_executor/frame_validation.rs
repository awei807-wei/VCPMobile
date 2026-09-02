use super::attachment_canonicalization::{canonicalize_attachment, BoundedWarnings};
use crate::vcp_modules::sync::sync_error::{parse_wire_sync_error, WireSyncError};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct TopicNDJSONFrame {
    pub(crate) topic_id: String,
    pub(crate) owner_type: Option<String>,
    pub(crate) owner_id: Option<String>,
    pub(crate) messages: Vec<crate::vcp_modules::sync_dto::MessagePullSyncDTO>,
    pub(crate) error: Option<WireSyncError>,
    pub(crate) legacy_attachment_warnings: usize,
    pub(crate) warning_samples: Vec<String>,
}

/// Parse a single Wire 1.2 topic frame and retain only canonical pull data.
pub(crate) fn parse_topic_ndjson_frame(bytes: &[u8]) -> Result<TopicNDJSONFrame, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("Malformed NDJSON frame: {error}"))?;
    let value =
        parse_strict_json(text).map_err(|error| format!("Malformed NDJSON frame: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "NDJSON frame must be an object".to_string())?;
    validate_frame_fields(object)?;
    let topic_id = require_topic_id(object.get("topicId"))?;
    let owner_identity = parse_owner_identity(object, &topic_id)?;
    let error = parse_error_frame(object, &topic_id)?;
    if let Some(error) = error {
        return Ok(TopicNDJSONFrame {
            topic_id,
            owner_type: owner_identity.as_ref().map(|identity| identity.0.clone()),
            owner_id: owner_identity.map(|identity| identity.1),
            messages: Vec::new(),
            error: Some(error),
            legacy_attachment_warnings: 0,
            warning_samples: Vec::new(),
        });
    }
    crate::vcp_modules::sync::wire_frame::parse_live_frame_value(value.clone())
        .map_err(|error| error.to_string())?;
    let raw_messages = match object.get("messages") {
        Some(Value::Array(messages)) => messages.clone(),
        _ => {
            return Err(format!(
                "NDJSON frame for {topic_id} requires messages array"
            ))
        }
    };
    if raw_messages.len() > super::MAX_NDJSON_ENTITIES {
        return Err(format!(
            "NDJSON frame for {topic_id} exceeds {} message budget",
            super::MAX_NDJSON_ENTITIES
        ));
    }
    let mut warnings = BoundedWarnings::default();
    read_upstream_warnings(object, &mut warnings, &topic_id)?;
    let messages = parse_messages(raw_messages, &topic_id, &mut warnings)?;
    Ok(TopicNDJSONFrame {
        topic_id,
        owner_type: owner_identity.as_ref().map(|identity| identity.0.clone()),
        owner_id: owner_identity.map(|identity| identity.1),
        messages,
        error: None,
        legacy_attachment_warnings: warnings.count,
        warning_samples: warnings.samples,
    })
}

fn validate_frame_fields(object: &Map<String, Value>) -> Result<(), String> {
    for field in object.keys() {
        if !matches!(
            field.as_str(),
            "topicId"
                | "ownerType"
                | "ownerId"
                | "messages"
                | "_error"
                | "legacyAttachmentWarnings"
                | "warningSamples"
        ) {
            return Err(format!("NDJSON frame contains unknown field {field}"));
        }
    }
    Ok(())
}

fn require_topic_id(value: Option<&Value>) -> Result<String, String> {
    match value {
        Some(Value::String(topic_id)) if !topic_id.is_empty() => Ok(topic_id.clone()),
        Some(Value::String(_)) => Err("NDJSON frame contains empty topicId".to_string()),
        _ => Err("NDJSON frame contains missing or non-string topicId".to_string()),
    }
}

fn parse_owner_identity(
    object: &Map<String, Value>,
    topic_id: &str,
) -> Result<Option<(String, String)>, String> {
    match (object.get("ownerType"), object.get("ownerId")) {
        (None | Some(Value::Null), None | Some(Value::Null)) => Ok(None),
        (Some(Value::String(owner_type)), Some(Value::String(owner_id)))
            if matches!(owner_type.as_str(), "agent" | "group") && !owner_id.is_empty() =>
        {
            Ok(Some((owner_type.clone(), owner_id.clone())))
        }
        _ => Err(format!(
            "NDJSON frame for {topic_id} requires valid ownerType and ownerId together"
        )),
    }
}

fn parse_error_frame(
    object: &Map<String, Value>,
    topic_id: &str,
) -> Result<Option<WireSyncError>, String> {
    let Some(raw_error) = object.get("_error") else {
        return Ok(None);
    };
    if raw_error.is_null() {
        return Ok(None);
    }
    if object.contains_key("legacyAttachmentWarnings") || object.contains_key("warningSamples") {
        return Err(format!(
            "NDJSON error frame for {topic_id} contains warning envelope fields"
        ));
    }
    if object
        .get("messages")
        .is_some_and(|messages| !matches!(messages, Value::Array(values) if values.is_empty()))
    {
        return Err(format!(
            "NDJSON error frame for {topic_id} must not contain live messages"
        ));
    }
    parse_wire_sync_error(raw_error)
        .map(Some)
        .map_err(|error| format!("NDJSON error frame for {topic_id} is invalid: {error}"))
}

fn read_upstream_warnings(
    object: &Map<String, Value>,
    warnings: &mut BoundedWarnings,
    topic_id: &str,
) -> Result<(), String> {
    if let Some(value) = object.get("legacyAttachmentWarnings") {
        if !value.is_null() {
            let count = value.as_u64().ok_or_else(|| {
                format!(
                    "NDJSON frame {topic_id} legacyAttachmentWarnings must be a non-negative integer"
                )
            })?;
            warnings.count = usize::try_from(count).map_err(|_| {
                format!("NDJSON frame {topic_id} legacyAttachmentWarnings exceeds platform limit")
            })?;
        }
    }
    if let Some(value) = object.get("warningSamples") {
        let samples = value.as_array().ok_or_else(|| {
            format!("NDJSON frame {topic_id} warningSamples must contain strings")
        })?;
        warnings.samples = samples
            .iter()
            .map(|sample| {
                sample.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    format!("NDJSON frame {topic_id} warningSamples must contain strings")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        warnings.samples.truncate(super::MAX_WARNING_SAMPLES);
    }
    Ok(())
}

fn parse_messages(
    raw_messages: Vec<Value>,
    topic_id: &str,
    warnings: &mut BoundedWarnings,
) -> Result<Vec<crate::vcp_modules::sync_dto::MessagePullSyncDTO>, String> {
    let mut messages = Vec::with_capacity(raw_messages.len());
    for raw_message in raw_messages {
        messages.push(parse_message(raw_message, topic_id, warnings)?);
    }
    Ok(messages)
}

fn parse_message(
    raw_message: Value,
    topic_id: &str,
    warnings: &mut BoundedWarnings,
) -> Result<crate::vcp_modules::sync_dto::MessagePullSyncDTO, String> {
    let mut message = match raw_message {
        Value::Object(message) => message,
        _ => return Err(format!("Topic {topic_id} contains a non-object message")),
    };
    let message_id = message
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| format!("Topic {topic_id} contains a message with missing or empty id"))?
        .to_string();
    message
        .get("role")
        .and_then(Value::as_str)
        .filter(|role| !role.is_empty())
        .ok_or_else(|| format!("Message {message_id} has missing or empty role"))?;
    let timestamp = normalize_timestamp(message.get("timestamp"), &message_id)?;
    message.insert("timestamp".to_string(), Value::from(timestamp));
    message.remove("contentHash");
    message.remove("content_hash");
    canonicalize_message_attachments(&mut message, &message_id, warnings)?;
    serde_json::from_value(Value::Object(message))
        .map_err(|error| format!("Message {message_id} violates protocol 1.2: {error}"))
}

fn canonicalize_message_attachments(
    message: &mut Map<String, Value>,
    message_id: &str,
    warnings: &mut BoundedWarnings,
) -> Result<(), String> {
    match message.remove("attachments") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(attachments)) => {
            let mut canonical = Vec::with_capacity(attachments.len());
            for (index, attachment) in attachments.into_iter().enumerate() {
                if let Some(attachment) =
                    canonicalize_attachment(attachment, message_id, index, warnings)?
                {
                    canonical.push(attachment);
                }
            }
            if !canonical.is_empty() {
                message.insert("attachments".to_string(), Value::Array(canonical));
            }
            Ok(())
        }
        Some(_) => Err(format!(
            "Message {message_id} attachments must be an array or null"
        )),
    }
}

fn normalize_timestamp(value: Option<&Value>, message_id: &str) -> Result<u64, String> {
    let timestamp = match value {
        Some(Value::Number(number)) => number.as_u64().ok_or_else(|| {
            format!("Message {message_id} timestamp must be a non-negative integer")
        }),
        Some(Value::String(timestamp)) => timestamp.parse::<u64>().map_err(|_| {
            format!("Message {message_id} timestamp string must be a non-negative integer")
        }),
        _ => Err(format!(
            "Message {message_id} timestamp must be a non-negative integer or integer string"
        )),
    }?;
    if timestamp > i64::MAX as u64 {
        return Err(format!(
            "Message {message_id} timestamp exceeds the SQLite integer range"
        ));
    }
    Ok(timestamp)
}

pub(crate) fn validate_returned_topic_identity(
    frame: &TopicNDJSONFrame,
    expected: &HashMap<String, (String, String)>,
) -> Result<(), String> {
    let Some((expected_owner_type, expected_owner_id)) = expected.get(&frame.topic_id) else {
        return Err(format!(
            "NDJSON returned unexpected topicId {}",
            frame.topic_id
        ));
    };
    if frame.owner_type.as_deref() != Some(expected_owner_type.as_str())
        || frame.owner_id.as_deref() != Some(expected_owner_id.as_str())
    {
        return Err(format!(
            "NDJSON topic {} owner identity conflicts with the local database",
            frame.topic_id
        ));
    }
    Ok(())
}

pub(crate) fn validate_requested_message_ids(
    topic_id: &str,
    expected: Option<&HashSet<String>>,
    messages: &[crate::vcp_modules::sync_dto::MessagePullSyncDTO],
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<HashSet<_>>();
    if actual == *expected {
        return Ok(());
    }
    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let mut unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "NDJSON message set mismatch for {topic_id}: missing={missing:?}, unexpected={unexpected:?}"
    ))
}
