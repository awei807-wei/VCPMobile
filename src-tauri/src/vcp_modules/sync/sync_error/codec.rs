use super::types::WireSyncError;
use super::validation::{parse_wire_sync_error, validate_wire_error};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use serde_json::Value;

pub const WIRE_ERROR_MARKER: &str = "SYNC_WIRE_ERROR:";

pub fn parse_wire_sync_error_frame(value: &Value) -> Result<WireSyncError, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "SYNC_ERROR frame must be an object".to_owned())?;
    for field in object.keys() {
        if !matches!(field.as_str(), "type" | "error") {
            return Err(format!("unknown SYNC_ERROR field {field}"));
        }
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
        .map_err(|serialize_error| format!("failed to encode Wire 1.2 error: {serialize_error}"))
}

pub fn encode_wire_sync_error_value(value: &Value) -> Result<String, String> {
    encode_wire_sync_error(&parse_wire_sync_error(value)?)
}

pub fn encode_http_sync_error_body(bytes: &[u8]) -> Result<Option<String>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("HTTP error body is not valid UTF-8: {error}"))?;
    let value = parse_strict_json(text)
        .map_err(|error| format!("HTTP error body is not strict JSON: {error}"))?;
    match value.get("error") {
        Some(error) => encode_wire_sync_error_value(error).map(Some),
        None => Ok(None),
    }
}

pub fn decode_wire_sync_error(text: &str) -> Option<WireSyncError> {
    let marker_offset = text.find(WIRE_ERROR_MARKER)? + WIRE_ERROR_MARKER.len();
    let value = parse_strict_json(&text[marker_offset..]).ok()?;
    parse_wire_sync_error(&value).ok()
}
