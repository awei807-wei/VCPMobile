use super::http::{http_transport_error, parse_strict_response, read_response_limited};
use super::types::{MessagePushFrame, PushBatchResult, MAX_NDJSON_LINE_BYTES, MAX_SYNC_BODY_BYTES};
use crate::vcp_modules::sync::sync_error::{encode_wire_sync_error, SyncErrorStage};
use crate::vcp_modules::sync::sync_types::MessagePushResponseFrame;
use crate::vcp_modules::topic_types::TopicKey;
use reqwest::Client;
use std::collections::HashSet;

pub(super) fn parse_message_push_frames(
    bytes: &[u8],
    expected_topics: &[TopicKey],
) -> Result<Vec<MessagePushFrame>, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("Message push response is not UTF-8: {error}"))?;
    let expected = validate_expected_topics(expected_topics)?;
    let mut seen = HashSet::new();
    let mut frames = Vec::with_capacity(expected.len());
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.len() > MAX_NDJSON_LINE_BYTES {
            return Err("Message push response contains a line over 32 MiB".to_string());
        }
        let value = parse_strict_response(line.as_bytes(), "Message push")?;
        let frame: MessagePushResponseFrame = serde_json::from_value(value)
            .map_err(|error| format!("Message push response frame is invalid: {error}"))?;
        frame.validate()?;
        frames.push(parse_response_frame(frame, &expected, &mut seen)?);
    }
    if seen != expected {
        let mut missing = expected.difference(&seen).cloned().collect::<Vec<_>>();
        missing.sort();
        return Err(format!(
            "Message push response is missing topics {missing:?}"
        ));
    }
    Ok(frames)
}

fn validate_expected_topics(expected: &[TopicKey]) -> Result<HashSet<TopicKey>, String> {
    let set = expected.iter().cloned().collect::<HashSet<_>>();
    if set.len() != expected.len() || expected.iter().any(|topic| !topic.is_valid()) {
        return Err("Message push expected topics must be unique and valid".to_string());
    }
    Ok(set)
}

fn parse_response_frame(
    frame: MessagePushResponseFrame,
    expected: &HashSet<TopicKey>,
    seen: &mut HashSet<TopicKey>,
) -> Result<MessagePushFrame, String> {
    let (owner_type, owner_id, topic_id, ok, error) = match frame {
        MessagePushResponseFrame::Topic {
            owner_type,
            owner_id,
            topic_id,
            ok,
            error,
        } => (owner_type, owner_id, topic_id, ok, error),
        MessagePushResponseFrame::StreamError { error } => {
            return Err(encode_wire_sync_error(&error)?);
        }
    };
    let topic = TopicKey::new(owner_type.as_str(), owner_id, topic_id);
    if !expected.contains(&topic) {
        return Err(format!(
            "Message push response contains unexpected topic {}",
            topic.topic_id
        ));
    }
    if !seen.insert(topic.clone()) {
        return Err(format!(
            "Message push response contains duplicate topic {}",
            topic.topic_id
        ));
    }
    let error = error.as_ref().map(encode_wire_sync_error).transpose()?;
    match (ok, error.as_ref()) {
        (true, None) | (false, Some(_)) => {}
        (true, Some(_)) => {
            return Err(format!(
                "Successful message push {} must not contain error",
                topic.topic_id
            ));
        }
        (false, None) => {
            return Err(format!(
                "Failed message push {} requires error",
                topic.topic_id
            ));
        }
    }
    Ok(MessagePushFrame {
        outcome: PushBatchResult {
            topic,
            success: ok,
            error,
        },
    })
}

pub(super) async fn send_message_chunk(
    client: &Client,
    http_url: &str,
    sync_token: &str,
    body: Vec<u8>,
    expected_topics: &[TopicKey],
) -> Result<Vec<MessagePushFrame>, String> {
    if body.len() > MAX_SYNC_BODY_BYTES {
        return Err("Message push request exceeds the 256 MiB body limit".to_string());
    }
    let response = client
        .post(format!("{http_url}/api/mobile-sync/messages/push"))
        .header("Authorization", format!("Bearer {sync_token}"))
        .header("Content-Type", "application/x-ndjson")
        .body(body)
        .send()
        .await
        .map_err(|error| {
            http_transport_error("Message push request", SyncErrorStage::Messages, &error)
        })?;
    let (status, bytes) = read_response_limited(
        response,
        MAX_SYNC_BODY_BYTES,
        "Message push",
        SyncErrorStage::Messages,
    )
    .await?;
    if !status.is_success() {
        let value = parse_strict_response(&bytes, "Message push")?;
        let object = value
            .as_object()
            .ok_or_else(|| "Message push HTTP error must be an object".to_string())?;
        if object.len() != 1 || !object.contains_key("error") {
            return Err("Message push HTTP error must contain exactly error".to_string());
        }
        let error = object.get("error").expect("checked above");
        return Err(crate::vcp_modules::sync::sync_error::encode_wire_sync_error_value(error)?);
    }
    parse_message_push_frames(&bytes, expected_topics)
}
