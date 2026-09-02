use super::http::{parse_strict_response, read_response_limited, require_exact_object_keys};
use super::types::{canonical_sha256, MessagePushFrame, PushBatchResult, MAX_NDJSON_LINE_BYTES};
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_error::encode_wire_sync_error_value;
use reqwest::Client;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(super) fn parse_message_push_frames(
    bytes: &[u8],
    expected_topic_ids: &[String],
) -> Result<Vec<MessagePushFrame>, String> {
    let response_text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Batch push response is not UTF-8: {error}"))?;
    let expected = expected_topic_ids.iter().cloned().collect::<HashSet<_>>();
    if expected.len() != expected_topic_ids.len() || expected_topic_ids.iter().any(String::is_empty)
    {
        return Err("Batch push expected topics must be unique and non-empty".to_string());
    }
    let mut seen = HashSet::new();
    let mut frames = Vec::with_capacity(expected.len());
    for raw_line in response_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_NDJSON_LINE_BYTES {
            return Err("Batch push response contains a line over 32 MiB".to_string());
        }
        frames.push(parse_message_push_line(line, &expected, &mut seen)?);
    }

    if seen != expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!("Batch push response is missing topics {missing:?}"));
    }
    Ok(frames)
}

fn parse_message_push_line(
    line: &str,
    expected: &HashSet<String>,
    seen: &mut HashSet<String>,
) -> Result<MessagePushFrame, String> {
    let data = parse_strict_json(line)
        .map_err(|error| format!("Batch push response contains malformed NDJSON: {error}"))?;
    if data.get("_stream_error").is_some() {
        return parse_stream_error_frame(&data);
    }
    let object = data
        .as_object()
        .ok_or_else(|| "Batch push response frame must be an object".to_string())?;
    let topic_id = object
        .get("topicId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Batch push response requires a non-empty topicId".to_string())?;
    if !expected.contains(topic_id) {
        return Err(format!(
            "Batch push response contains unexpected topic {topic_id}"
        ));
    }
    if !seen.insert(topic_id.to_string()) {
        return Err(format!(
            "Batch push response contains duplicate topic {topic_id}"
        ));
    }
    let success = object
        .get("success")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("Batch push result for {topic_id} requires boolean success"))?;
    let needed_attachment_hashes = parse_needed_hashes(object, topic_id)?;
    let error = parse_result_error(object, topic_id, success)?;
    validate_message_result_fields(object, topic_id, success, &needed_attachment_hashes)?;
    Ok(MessagePushFrame {
        outcome: PushBatchResult {
            topic_id: topic_id.to_string(),
            success,
            error,
        },
        needed_attachment_hashes,
    })
}

fn validate_message_result_fields(
    object: &serde_json::Map<String, Value>,
    topic_id: &str,
    success: bool,
    needed_attachment_hashes: &[String],
) -> Result<(), String> {
    let expected_fields = if success {
        ["topicId", "success", "neededAttachmentHashes", ""]
    } else {
        ["topicId", "success", "neededAttachmentHashes", "error"]
    };
    let expected_len = if success { 3 } else { 4 };
    if object.len() != expected_len
        || object.keys().any(|key| {
            !expected_fields[..expected_len]
                .iter()
                .any(|allowed| *allowed == key)
        })
    {
        return Err(format!(
            "Batch push result for {topic_id} has an unexpected or missing field"
        ));
    }
    if !success && !needed_attachment_hashes.is_empty() {
        return Err(format!(
            "Failed batch push result for {topic_id} must not request attachments"
        ));
    }
    Ok(())
}

fn parse_stream_error_frame(data: &Value) -> Result<MessagePushFrame, String> {
    let object = data
        .as_object()
        .ok_or_else(|| "Batch push stream error must be an object".to_string())?;
    if object.len() != 1 || !object.contains_key("_stream_error") {
        return Err("Batch push _stream_error wrapper has unexpected fields".to_string());
    }
    let error = object
        .get("_stream_error")
        .ok_or_else(|| "Batch push _stream_error wrapper is missing its error".to_string())?;
    Err(encode_wire_sync_error_value(error)?)
}

fn parse_needed_hashes(
    object: &serde_json::Map<String, Value>,
    topic_id: &str,
) -> Result<Vec<String>, String> {
    let values = object
        .get("neededAttachmentHashes")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!("neededAttachmentHashes for {topic_id} must be an explicit array")
        })?;
    let mut hashes = Vec::with_capacity(values.len());
    let mut unique_hashes = HashSet::new();
    for value in values {
        let raw_hash = value
            .as_str()
            .ok_or_else(|| format!("neededAttachmentHashes for {topic_id} must contain strings"))?;
        let hash = canonical_sha256(raw_hash).ok_or_else(|| {
            format!("neededAttachmentHashes for {topic_id} contains an invalid hash")
        })?;
        if !unique_hashes.insert(hash.clone()) {
            return Err(format!(
                "neededAttachmentHashes for {topic_id} contains duplicate hash {hash}"
            ));
        }
        hashes.push(hash);
    }
    Ok(hashes)
}

fn parse_result_error(
    object: &serde_json::Map<String, Value>,
    topic_id: &str,
    success: bool,
) -> Result<Option<String>, String> {
    let error = object.get("error");
    if success && error.is_some() {
        return Err(format!(
            "Successful batch push result for {topic_id} must not contain an error"
        ));
    }
    if !success {
        let error = error.ok_or_else(|| {
            format!("Failed batch push result for {topic_id} requires an error message")
        })?;
        return encode_wire_sync_error_value(error)
            .map(Some)
            .map_err(|parse_error| {
                format!("Batch push result for {topic_id} has invalid error: {parse_error}")
            });
    }
    Ok(None)
}

pub(super) async fn send_message_chunk(
    client: &Client,
    http_url: &str,
    sync_token: &str,
    body: Vec<u8>,
    expected_topic_ids: &[String],
) -> Result<Vec<MessagePushFrame>, String> {
    let url = format!("{http_url}/api/mobile-sync/upload-messages-batch");
    let response = client
        .post(&url)
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/x-ndjson")
        .body(body)
        .send()
        .await
        .map_err(|error| format!("Batch push request failed: {error}"))?;
    let (status, bytes) =
        read_response_limited(response, MAX_NDJSON_LINE_BYTES, "Batch push").await?;
    if !status.is_success() {
        let value = parse_strict_response(&bytes, "Batch push")?;
        let object = require_exact_object_keys(&value, &["error"], "Batch push")?;
        let error = object.get("error").ok_or_else(|| {
            format!("Batch push messages failed with HTTP {status} without an error")
        })?;
        return Err(encode_wire_sync_error_value(error).map_err(|error| {
            format!("Batch push messages returned an invalid Wire 1.2 error: {error}")
        })?);
    }
    parse_message_push_frames(&bytes, expected_topic_ids)
}

pub(super) fn record_message_frames(
    frames: Vec<MessagePushFrame>,
    results: &mut Vec<PushBatchResult>,
    attachment_topics: &mut HashMap<String, HashSet<String>>,
) {
    for frame in frames {
        if frame.outcome.success {
            for hash in frame.needed_attachment_hashes {
                attachment_topics
                    .entry(hash)
                    .or_default()
                    .insert(frame.outcome.topic_id.clone());
            }
        }
        results.push(frame.outcome);
    }
}
