use super::registry::{error_definition, fallback_definition};
use super::types::{SyncErrorStage, WireSyncError, MAX_ERROR_MESSAGE_CHARS};
use super::validation::{is_valid_wire_code, sanitize_topic_ids};
use super::validation::{parse_wire_sync_error, validate_wire_error};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use serde_json::Value;

pub const WIRE_ERROR_MARKER: &str = "SYNC_WIRE_ERROR:";

/// Errors that can safely restart a complete attempt after transport/state
/// recovery. Protocol and storage failures must never enter this path.
pub fn is_attempt_restart_code(code: &str) -> bool {
    matches!(code, "HTTP_TRANSPORT_FAILED" | "SYNC_SNAPSHOT_STALE")
}

pub fn parse_wire_sync_error_frame(value: &Value) -> Result<WireSyncError, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "SYNC_ERROR frame must be an object".to_owned())?;
    if object.len() != 2
        || object
            .keys()
            .any(|field| !matches!(field.as_str(), "type" | "error"))
    {
        return Err("SYNC_ERROR frame must contain exactly type and error".to_owned());
    }
    if object.get("type").and_then(Value::as_str) != Some("SYNC_ERROR") {
        return Err("expected SYNC_ERROR frame".to_owned());
    }
    let error = object
        .get("error")
        .ok_or_else(|| "SYNC_ERROR requires error object".to_owned())?;
    parse_wire_sync_error(error)
}

pub fn encode_wire_sync_error(error: &WireSyncError) -> Result<String, String> {
    let validated = validate_wire_error(error.clone())?;
    serde_json::to_string(&validated)
        .map(|json| format!("{WIRE_ERROR_MARKER}{json}"))
        .map_err(|serialize_error| format!("failed to encode Wire 1.4 error: {serialize_error}"))
}

/// Encodes a local failure for internal propagation without exposing a
/// platform errno as a public Wire error code.
pub fn encode_local_sync_error(
    code: &str,
    stage: SyncErrorStage,
    message: &str,
    failed_topic_ids: Vec<String>,
) -> String {
    let stable_code = if is_valid_wire_code(code) {
        code
    } else {
        "SYNC_ATTEMPT_FAILED"
    };
    let selected = error_definition(stable_code).unwrap_or_else(fallback_definition);
    let bounded_message = message
        .trim()
        .chars()
        .take(MAX_ERROR_MESSAGE_CHARS)
        .collect::<String>();
    let wire = WireSyncError {
        code: stable_code.to_owned(),
        origin: super::types::SyncErrorOrigin::MobileSync,
        stage,
        kind: selected.category,
        retry: selected.retry,
        message: if bounded_message.is_empty() {
            "sync operation failed".to_owned()
        } else {
            bounded_message
        },
        failed_topic_ids: sanitize_topic_ids(failed_topic_ids),
    };
    encode_wire_sync_error(&wire).unwrap_or_else(|_| {
        format!(
            "{WIRE_ERROR_MARKER}{}",
            r#"{"code":"SYNC_ATTEMPT_FAILED","origin":"mobile_sync","stage":"startup","kind":"internal","retry":"manual","message":"failed to encode local sync error","failedTopicIds":[]}"#
        )
    })
}

pub fn encode_wire_sync_error_value(value: &Value) -> Result<String, String> {
    encode_wire_sync_error(&parse_wire_sync_error(value)?)
}

pub fn encode_http_sync_error_body(bytes: &[u8]) -> Result<Option<String>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("HTTP error body is not valid UTF-8: {error}"))?;
    let value = parse_strict_json(text)
        .map_err(|error| format!("HTTP error body is not strict JSON: {error}"))?;
    let Some(object) = value.as_object() else {
        return Err("HTTP error body must be an object".to_owned());
    };
    let Some(error) = object.get("error") else {
        return Ok(None);
    };
    if object.len() != 1 {
        return Err("HTTP error body must contain exactly error".to_owned());
    }
    encode_wire_sync_error_value(error).map(Some)
}

pub fn decode_wire_sync_error(text: &str) -> Option<WireSyncError> {
    let marker_offset = text.find(WIRE_ERROR_MARKER)? + WIRE_ERROR_MARKER.len();
    let value = parse_strict_json(&text[marker_offset..]).ok()?;
    parse_wire_sync_error(&value).ok()
}
