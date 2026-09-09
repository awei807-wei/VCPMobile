use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::vcp_modules::sync::wire_protocol::parse_strict_json;

pub(super) fn parse_business_frame(text: &str) -> Result<Value, (&'static str, String)> {
    let payload = parse_strict_json(text).map_err(|error| {
        let message = error.to_string();
        let code = if message.contains("duplicate JSON object key") {
            "PROTOCOL_DUPLICATE_KEY"
        } else {
            "PROTOCOL_FRAME_INVALID"
        };
        (code, format!("Malformed WebSocket frame: {message}"))
    })?;
    let object = payload.as_object().ok_or((
        "PROTOCOL_FRAME_INVALID",
        "WebSocket frame must be an object".to_string(),
    ))?;
    object
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or((
            "PROTOCOL_FRAME_INVALID",
            "WebSocket frame requires a non-empty string type".to_string(),
        ))?;
    Ok(payload)
}

pub(super) fn parse_typed<T: DeserializeOwned>(payload: Value, label: &str) -> Result<T, String> {
    serde_json::from_value(payload).map_err(|error| format!("Invalid {label}: {error}"))
}
