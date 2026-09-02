use crate::vcp_modules::sync::sync_error::{
    encode_http_sync_error_body, encode_wire_sync_error_value,
};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use futures_util::StreamExt;

/// Read a bounded HTTP response body without trusting Content-Length alone.
pub(crate) async fn read_response_limited(
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

pub(crate) fn http_status_error(
    operation: &str,
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> String {
    match encode_http_sync_error_body(bytes) {
        Ok(Some(encoded)) => encoded,
        Ok(None) => {
            format!("{operation} failed with HTTP {status} without a Wire 1.2 error object")
        }
        Err(error) => format!("{operation} returned an invalid Wire 1.2 error: {error}"),
    }
}

/// Parse a stream-level error line using the same strict JSON decoder as Wire 1.2 frames.
pub(crate) fn parse_stream_error_frame(bytes: &[u8]) -> Result<Option<String>, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| format!("Malformed NDJSON frame: {error}"))?;
    let value =
        parse_strict_json(text).map_err(|error| format!("Malformed NDJSON frame: {error}"))?;
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let Some(error) = object.get("_stream_error") else {
        return Ok(None);
    };
    if object.len() != 1 {
        return Err("NDJSON _stream_error envelope contains unknown fields".to_string());
    }
    if error.is_null() {
        return Err("NDJSON _stream_error must be a Wire 1.2 error object".to_string());
    }
    encode_wire_sync_error_value(error).map(Some)
}

pub(crate) struct NdjsonBudget {
    max_frames: usize,
    total_bytes: usize,
    frames: usize,
    entities: usize,
}

impl NdjsonBudget {
    pub(crate) fn new(max_frames: usize) -> Self {
        Self {
            max_frames,
            total_bytes: 0,
            frames: 0,
            entities: 0,
        }
    }

    pub(crate) fn observe_chunk(&mut self, bytes: usize) -> Result<(), String> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .ok_or_else(|| "NDJSON response size overflow".to_string())?;
        if self.total_bytes > super::MAX_NDJSON_TOTAL_BYTES {
            return Err("NDJSON response exceeds 256MB budget".to_string());
        }
        Ok(())
    }

    pub(crate) fn observe_frame(
        &mut self,
        line_bytes: usize,
        entities: usize,
    ) -> Result<(), String> {
        if line_bytes > super::MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32MB budget".to_string());
        }
        self.frames += 1;
        if self.frames > self.max_frames {
            return Err("NDJSON response contains more frames than requested topics".to_string());
        }
        self.entities = self
            .entities
            .checked_add(entities)
            .ok_or_else(|| "NDJSON entity count overflow".to_string())?;
        if self.entities > super::MAX_NDJSON_ENTITIES {
            return Err("NDJSON response exceeds 100000 message budget".to_string());
        }
        Ok(())
    }
}

pub(crate) fn pull_worker_permits(frame_bytes: usize) -> Result<u32, String> {
    let units = frame_bytes.saturating_add(super::PULL_WORKER_BUDGET_UNIT_BYTES - 1)
        / super::PULL_WORKER_BUDGET_UNIT_BYTES;
    u32::try_from(units.max(1)).map_err(|_| "Pull worker permit count overflow".to_string())
}
