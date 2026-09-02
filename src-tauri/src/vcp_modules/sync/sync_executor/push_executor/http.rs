use super::types::MAX_CONTROL_RESPONSE_BYTES;
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_error::encode_wire_sync_error_value;
use futures_util::StreamExt;
use serde_json::{Map, Value};

pub(super) async fn read_response_limited(
    response: reqwest::Response,
    max_bytes: usize,
    operation: &str,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("{operation} response exceeds {max_bytes} bytes"));
    }
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("{operation} response read failed: {error}"))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!("{operation} response exceeds {max_bytes} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

pub(super) fn parse_strict_response(bytes: &[u8], operation: &str) -> Result<Value, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{operation} response is not UTF-8: {error}"))?;
    parse_strict_json(text).map_err(|error| format!("{operation} returned invalid JSON: {error}"))
}

pub(super) fn require_exact_object_keys<'a>(
    value: &'a Value,
    required: &[&str],
    operation: &str,
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{operation} response must be an object"))?;
    let has_exact_keys = object.len() == required.len()
        && object
            .keys()
            .all(|key| required.iter().any(|allowed| *allowed == key));
    if !has_exact_keys {
        return Err(format!(
            "{operation} response fields do not exactly match {:?}",
            required
        ));
    }
    Ok(object)
}

pub(super) async fn parse_success_response(
    response: reqwest::Response,
    operation: &str,
) -> Result<Value, String> {
    let (status, bytes) =
        read_response_limited(response, MAX_CONTROL_RESPONSE_BYTES, operation).await?;
    if !status.is_success() {
        let value = parse_strict_response(&bytes, operation)?;
        let object = require_exact_object_keys(&value, &["error"], operation)?;
        let error = object
            .get("error")
            .ok_or_else(|| format!("{operation} failed with HTTP {status} without an error"))?;
        return Err(encode_wire_sync_error_value(error).map_err(|error| {
            format!("{operation} returned an invalid Wire 1.2 error: {error}")
        })?);
    }

    let value = parse_strict_response(&bytes, operation)?;
    let success = value.get("success").and_then(Value::as_bool);
    if success != Some(true) {
        let error = value.get("error").or_else(|| {
            value
                .get("results")
                .and_then(Value::as_array)
                .and_then(|results| {
                    results.iter().find_map(|result| {
                        (result.get("success").and_then(Value::as_bool) == Some(false))
                            .then(|| result.get("error"))
                            .flatten()
                    })
                })
        });
        let encoded = error
            .ok_or_else(|| format!("{operation} returned success=false without error"))
            .and_then(encode_wire_sync_error_value)?;
        return Err(encoded);
    }
    if value.get("error").is_some() {
        return Err(format!(
            "{operation} returned success=true together with error"
        ));
    }
    Ok(value)
}
