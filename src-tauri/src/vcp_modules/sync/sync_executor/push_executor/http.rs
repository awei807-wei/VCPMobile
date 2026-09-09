use super::types::MAX_CONTROL_RESPONSE_BYTES;
use crate::vcp_modules::sync::sync_error::{
    encode_http_sync_error_body, encode_local_sync_error, SyncErrorStage,
};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use futures_util::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub(super) async fn read_response_limited(
    response: reqwest::Response,
    max_bytes: usize,
    operation: &str,
    stage: SyncErrorStage,
) -> Result<(reqwest::StatusCode, Vec<u8>), String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(encode_local_sync_error(
            "RESPONSE_TOO_LARGE",
            stage,
            &format!("{operation} response exceeds {max_bytes} bytes"),
            Vec::new(),
        ));
    }
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            encode_local_sync_error(
                "HTTP_TRANSPORT_FAILED",
                stage,
                &format!("{operation} response read failed: {error}"),
                Vec::new(),
            )
        })?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err(encode_local_sync_error(
                "RESPONSE_TOO_LARGE",
                stage,
                &format!("{operation} response exceeds {max_bytes} bytes"),
                Vec::new(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, body))
}

pub(super) fn protocol_error(stage: SyncErrorStage, message: impl AsRef<str>) -> String {
    encode_local_sync_error("SYNC_PROTOCOL_INVALID", stage, message.as_ref(), Vec::new())
}

pub(super) fn http_transport_error(
    operation: &str,
    stage: SyncErrorStage,
    error: &reqwest::Error,
) -> String {
    encode_local_sync_error(
        "HTTP_TRANSPORT_FAILED",
        stage,
        &format!("{operation} failed: {error}"),
        Vec::new(),
    )
}

pub(super) async fn parse_json_response<T: DeserializeOwned>(
    response: reqwest::Response,
    operation: &str,
    stage: SyncErrorStage,
) -> Result<T, String> {
    let (status, bytes) =
        read_response_limited(response, MAX_CONTROL_RESPONSE_BYTES, operation, stage).await?;
    if !status.is_success() {
        return parse_http_error(&bytes, status, operation, stage);
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        protocol_error(stage, format!("{operation} response is not UTF-8: {error}"))
    })?;
    let value = parse_strict_json(text).map_err(|error| {
        protocol_error(stage, format!("{operation} returned invalid JSON: {error}"))
    })?;
    serde_json::from_value(value).map_err(|error| {
        protocol_error(
            stage,
            format!("{operation} returned invalid response: {error}"),
        )
    })
}

fn parse_http_error<T>(
    bytes: &[u8],
    status: reqwest::StatusCode,
    operation: &str,
    stage: SyncErrorStage,
) -> Result<T, String> {
    match encode_http_sync_error_body(bytes) {
        Ok(Some(error)) => Err(error),
        Ok(None) => Err(protocol_error(
            stage,
            format!("{operation} failed with HTTP {status} without a Wire 1.4 error object"),
        )),
        Err(error) => Err(protocol_error(
            stage,
            format!("{operation} returned an invalid Wire 1.4 error: {error}"),
        )),
    }
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
    if object.len() != required.len()
        || object
            .keys()
            .any(|key| !required.iter().any(|allowed| *allowed == key))
    {
        return Err(format!(
            "{operation} response fields do not exactly match {:?}",
            required
        ));
    }
    Ok(object)
}
