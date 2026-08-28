//! Wire 1.2 入站消息帧契约。

use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;

/// 解析后的 live topic frame。
///
/// 消息保留为 JSON 值，附件白名单和 canonical hash 在 B2.3 的边界内处理。
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFrame {
    pub topic_id: String,
    pub messages: Vec<Value>,
}

/// A strictly decoded post-handshake WebSocket frame.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundFrame {
    pub frame_type: String,
    pub payload: Value,
}

/// Wire frame 解析失败时返回的稳定错误信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireFrameError {
    pub code: &'static str,
    pub message: String,
}

impl WireFrameError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "PROTOCOL_FRAME_INVALID",
            message: message.into(),
        }
    }

    fn duplicate_key(message: impl Into<String>) -> Self {
        Self {
            code: "PROTOCOL_DUPLICATE_KEY",
            message: message.into(),
        }
    }
}

impl fmt::Display for WireFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WireFrameError {}

/// Parse one NDJSON live frame while rejecting malformed JSON and duplicate keys.
pub fn parse_live_frame(bytes: &[u8]) -> Result<LiveFrame, WireFrameError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| WireFrameError::invalid(format!("Malformed NDJSON frame: {error}")))?;
    let parsed = parse_wire_json(text, "NDJSON")?;
    parse_live_frame_value(parsed)
}

/// Strictly decodes one post-handshake WebSocket frame and requires a type.
pub fn parse_inbound_frame(text: &str) -> Result<InboundFrame, WireFrameError> {
    let payload = parse_wire_json(text, "WebSocket")?;
    let object = payload
        .as_object()
        .ok_or_else(|| WireFrameError::invalid("WebSocket frame must be an object"))?;
    let frame_type = object
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| WireFrameError::invalid("WebSocket frame requires a non-empty string type"))?
        .to_owned();
    Ok(InboundFrame {
        frame_type,
        payload,
    })
}

fn parse_wire_json(text: &str, context: &str) -> Result<Value, WireFrameError> {
    parse_strict_json(text).map_err(|error| {
        let message = error.to_string();
        if message.contains("duplicate JSON object key") {
            WireFrameError::duplicate_key(message)
        } else {
            WireFrameError::invalid(format!("Malformed {context} frame: {message}"))
        }
    })
}

/// Validate a decoded JSON value as a Wire 1.2 live topic frame.
pub fn parse_live_frame_value(value: Value) -> Result<LiveFrame, WireFrameError> {
    let object = match value {
        Value::Object(object) => object,
        _ => return Err(WireFrameError::invalid("NDJSON frame must be an object")),
    };
    let topic_id = require_frame_topic_id(object.get("topicId"))?;
    let messages = match object.get("messages") {
        Some(Value::Array(messages)) => messages.clone(),
        _ => {
            return Err(WireFrameError::invalid(format!(
                "NDJSON frame for {topic_id} requires messages array"
            )))
        }
    };

    let mut seen_message_ids = HashSet::with_capacity(messages.len());
    for message in &messages {
        validate_live_message(&topic_id, message, &mut seen_message_ids)?;
    }
    Ok(LiveFrame { topic_id, messages })
}

fn require_frame_topic_id(value: Option<&Value>) -> Result<String, WireFrameError> {
    match value {
        Some(Value::String(topic_id)) if !topic_id.is_empty() => Ok(topic_id.clone()),
        Some(Value::String(_)) => Err(WireFrameError::invalid(
            "NDJSON frame contains empty topicId",
        )),
        _ => Err(WireFrameError::invalid(
            "NDJSON frame contains missing or non-string topicId",
        )),
    }
}

fn validate_live_message(
    topic_id: &str,
    value: &Value,
    seen_message_ids: &mut HashSet<String>,
) -> Result<(), WireFrameError> {
    let object = match value {
        Value::Object(object) => object,
        _ => {
            return Err(WireFrameError::invalid(format!(
                "Topic {topic_id} contains a non-object message"
            )))
        }
    };
    let message_id = match object.get("id") {
        Some(Value::String(id)) if !id.is_empty() => id.clone(),
        Some(Value::String(_)) => {
            return Err(WireFrameError::invalid(format!(
                "Topic {topic_id} contains a message with empty id"
            )))
        }
        _ => {
            return Err(WireFrameError::invalid(format!(
                "Topic {topic_id} contains a message with missing or non-string id"
            )))
        }
    };
    if !seen_message_ids.insert(message_id.clone()) {
        return Err(WireFrameError::invalid(format!(
            "Topic {topic_id} contains duplicate message {message_id}"
        )));
    }
    validate_message_topic(topic_id, &message_id, object.get("topicId"))?;
    if object.get("status") == Some(&Value::String("removed".to_owned()))
        || object
            .get("deletedAt")
            .is_some_and(|deleted_at| !deleted_at.is_null())
    {
        return Err(WireFrameError::invalid(format!(
            "Tombstoned message {message_id} must not appear in a live pull frame"
        )));
    }
    Ok(())
}

fn validate_message_topic(
    frame_topic_id: &str,
    message_id: &str,
    value: Option<&Value>,
) -> Result<(), WireFrameError> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(topic_id)) if topic_id == frame_topic_id => Ok(()),
        Some(Value::String(topic_id)) => Err(WireFrameError::invalid(format!(
            "Message {message_id} topicId {topic_id} conflicts with frame topic {frame_topic_id}"
        ))),
        Some(_) => Err(WireFrameError::invalid(format!(
            "Message {message_id} topicId must be a string"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const PROTOCOL_FIXTURE_SHA256: &str =
        "7226118ea55766f952575032efc8cfff883a19c9d196f637ac267cb8795fcef8";

    fn fixture() -> Value {
        serde_json::from_slice(include_bytes!("../fixtures/protocol_1_2_golden.json"))
            .expect("protocol fixture must be valid JSON")
    }

    #[test]
    fn protocol_fixture_is_byte_exact_and_valid_frame_is_accepted() {
        let bytes = include_bytes!("../fixtures/protocol_1_2_golden.json");
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            PROTOCOL_FIXTURE_SHA256
        );

        let entry = &fixture()["validFrames"][0];
        let frame = parse_live_frame_value(entry["input"].clone()).expect("valid frame");
        assert_eq!(frame.topic_id, "topic-golden");
        assert_eq!(frame.messages.len(), 2);
    }

    #[test]
    fn fixture_invalid_frames_match_their_fail_closed_errors() {
        let root = fixture();
        for entry in root["invalidFrames"].as_array().expect("invalidFrames") {
            let error = parse_live_frame_value(entry["input"].clone()).expect_err("invalid frame");
            assert!(error
                .to_string()
                .contains(entry["errorContains"].as_str().unwrap()));
        }
    }

    #[test]
    fn frame_topic_id_must_be_non_empty_string() {
        for topic_id in [json!(null), json!(""), json!(42)] {
            let error = parse_live_frame_value(json!({ "topicId": topic_id, "messages": [] }))
                .expect_err("invalid frame topic");
            assert!(error.to_string().contains("topicId"));
        }
    }

    #[test]
    fn messages_must_be_array_and_message_ids_must_be_unique_non_empty_strings() {
        for messages in [json!(null), json!({}), json!("messages")] {
            assert!(
                parse_live_frame_value(json!({ "topicId": "t", "messages": messages })).is_err()
            );
        }

        for messages in [
            json!([{}]),
            json!([{ "id": "" }]),
            json!([{ "id": 7 }]),
            json!([{ "id": "same" }, { "id": "same" }]),
        ] {
            assert!(
                parse_live_frame_value(json!({ "topicId": "t", "messages": messages })).is_err()
            );
        }
    }

    #[test]
    fn message_topic_id_missing_or_null_is_allowed_but_present_value_must_match() {
        for topic_id in [None, Some(Value::Null), Some(json!("topic"))] {
            let mut message = json!({ "id": "m" });
            if let Some(topic_id) = topic_id {
                message["topicId"] = topic_id;
            }
            parse_live_frame_value(json!({ "topicId": "topic", "messages": [message] }))
                .expect("message topic metadata is allowed");
        }

        for topic_id in [json!("other"), json!(42), json!(false)] {
            let error = parse_live_frame_value(json!({
                "topicId": "topic",
                "messages": [{ "id": "m", "topicId": topic_id }]
            }))
            .expect_err("conflicting or non-string message topic must fail closed");
            assert!(error.to_string().contains("topicId"));
        }
    }

    #[test]
    fn tombstoned_live_messages_are_rejected() {
        for message in [
            json!({ "id": "removed", "status": "removed" }),
            json!({ "id": "deleted", "deletedAt": 1700000004 }),
        ] {
            let error = parse_live_frame_value(json!({
                "topicId": "topic-tombstone",
                "messages": [message]
            }))
            .expect_err("tombstoned message must be rejected");
            assert!(error.to_string().contains("Tombstoned message"));
        }
    }

    #[test]
    fn duplicate_json_keys_are_rejected_before_frame_validation() {
        let error = parse_live_frame(br#"{"topicId":"t","topicId":"t","messages":[]}"#)
            .expect_err("duplicate key");
        assert_eq!(error.code, "PROTOCOL_DUPLICATE_KEY");
    }

    #[test]
    fn inbound_frames_require_one_object_with_non_empty_string_type() {
        let frame = parse_inbound_frame(r#"{"type":"SYNC_LOG_EVENT","message":"ok"}"#)
            .expect("valid inbound frame");
        assert_eq!(frame.frame_type, "SYNC_LOG_EVENT");

        for payload in [
            r#"null"#,
            r#"[]"#,
            r#"{}"#,
            r#"{"type":""}"#,
            r#"{"type":7}"#,
            r#"{"type":"SYNC_LOG_EVENT"} trailing"#,
        ] {
            assert!(parse_inbound_frame(payload).is_err(), "invalid {payload}");
        }
    }

    #[test]
    fn inbound_frames_reject_duplicate_keys_at_every_depth() {
        for payload in [
            r#"{"type":"SYNC_LOG_EVENT","type":"SYNC_ERROR"}"#,
            r#"{"type":"SYNC_ERROR","error":{"code":"A","code":"B"}}"#,
        ] {
            let error = parse_inbound_frame(payload).expect_err("duplicate key");
            assert_eq!(error.code, "PROTOCOL_DUPLICATE_KEY");
        }
    }
}
