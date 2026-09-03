use super::avatar_pulls::validate_avatar_bytes;
use super::frame_validation::{
    parse_topic_ndjson_frame, validate_requested_message_ids, validate_returned_topic_identity,
};
use super::ndjson_codec::parse_stream_error_frame;
use super::ndjson_stream::NdjsonLineBuffer;
use super::result_reporting::progress_payload;
use super::{
    PullProgressContext, MAX_NDJSON_ENTITIES, MAX_NDJSON_LINE_BYTES, MAX_NDJSON_TOTAL_BYTES,
};
use serde_json::{json, Value};
use std::collections::HashSet;

const MAX_SAFE_JSON_INTEGER: u64 = crate::vcp_modules::sync::sync_types::MAX_SAFE_TIMESTAMP as u64;

fn message(id: &str) -> Value {
    json!({
        "id": id,
        "role": "user",
        "content": "你好",
        "timestamp": 1,
        "updatedAt": 2,
    })
}

fn topic_frame(topic: &str, owner_type: &str, owner_id: &str, messages: Value) -> Value {
    json!({
        "kind": "topic",
        "topicId": topic,
        "ownerType": owner_type,
        "ownerId": owner_id,
        "ok": true,
        "messages": messages,
    })
}

fn topic_key(
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
) -> crate::vcp_modules::topic_types::TopicKey {
    crate::vcp_modules::topic_types::TopicKey::new(owner_type, owner_id, topic_id)
}

#[test]
fn wire14_topic_frame_requires_full_identity_and_canonical_updated_at() {
    let frame = parse_topic_ndjson_frame(
        &serde_json::to_vec(&topic_frame(
            "shared-topic",
            "group",
            "group-a",
            json!([message("shared-message")]),
        ))
        .unwrap(),
    )
    .expect("valid Wire 1.4 topic frame");
    assert_eq!(frame.topic, topic_key("group", "group-a", "shared-topic"));
    assert_eq!(frame.messages[0].updated_at, 2);

    for missing in ["kind", "topicId", "ownerType", "ownerId", "ok", "messages"] {
        let mut value = topic_frame("topic", "agent", "agent-a", json!([]));
        value.as_object_mut().unwrap().remove(missing);
        assert!(
            parse_topic_ndjson_frame(&serde_json::to_vec(&value).unwrap()).is_err(),
            "missing {missing}"
        );
    }

    let missing_warning_pair = topic_frame("topic", "agent", "agent-a", json!([]));
    let mut missing_warning_pair = missing_warning_pair;
    missing_warning_pair["legacyAttachmentWarnings"] = json!(1);
    assert!(parse_topic_ndjson_frame(&serde_json::to_vec(&missing_warning_pair).unwrap()).is_err());
}

#[test]
fn wire14_message_numeric_fields_accept_max_safe_and_reject_two_to_the_53rd() {
    let mut safe = message("safe");
    safe["timestamp"] = json!(MAX_SAFE_JSON_INTEGER);
    safe["updatedAt"] = json!(MAX_SAFE_JSON_INTEGER);
    safe["attachments"] = json!([{
        "type": "file",
        "name": "payload.bin",
        "size": MAX_SAFE_JSON_INTEGER,
        "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "createdAt": MAX_SAFE_JSON_INTEGER
    }]);
    parse_topic_ndjson_frame(
        &serde_json::to_vec(&topic_frame("topic", "agent", "agent-a", json!([safe]))).unwrap(),
    )
    .expect("Wire 1.4 accepts JavaScript's largest safe integer");

    for field in ["timestamp", "updatedAt"] {
        let mut unsafe_message = message("unsafe");
        unsafe_message[field] = json!(MAX_SAFE_JSON_INTEGER + 1);
        assert!(
            parse_topic_ndjson_frame(
                &serde_json::to_vec(&topic_frame(
                    "topic",
                    "agent",
                    "agent-a",
                    json!([unsafe_message]),
                ))
                .unwrap(),
            )
            .is_err(),
            "2^53 must be rejected for {field}"
        );
    }

    for field in ["size", "createdAt"] {
        let mut unsafe_message = message("unsafe-attachment");
        unsafe_message["attachments"] = json!([{
            "type": "file",
            "name": "payload.bin",
            "size": MAX_SAFE_JSON_INTEGER,
            "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "createdAt": MAX_SAFE_JSON_INTEGER
        }]);
        unsafe_message["attachments"][0][field] = json!(MAX_SAFE_JSON_INTEGER + 1);
        assert!(
            parse_topic_ndjson_frame(
                &serde_json::to_vec(&topic_frame(
                    "topic",
                    "agent",
                    "agent-a",
                    json!([unsafe_message]),
                ))
                .unwrap(),
            )
            .is_err(),
            "2^53 must be rejected for attachment {field}"
        );
    }
}

#[test]
fn wire14_frame_rejects_local_wire_fields_and_tombstones() {
    let mut local = message("local");
    local["src"] = json!("file:///private/path");
    assert!(parse_topic_ndjson_frame(
        &serde_json::to_vec(&topic_frame("topic", "agent", "agent-a", json!([local]))).unwrap()
    )
    .is_err());

    let mut tombstone = message("removed");
    tombstone["deletedAt"] = json!(7);
    assert!(parse_topic_ndjson_frame(
        &serde_json::to_vec(&topic_frame(
            "topic",
            "agent",
            "agent-a",
            json!([tombstone]),
        ))
        .unwrap()
    )
    .is_err());
}

#[test]
fn full_topic_and_message_identity_isolation_is_enforced() {
    let agent = topic_frame(
        "shared-topic",
        "agent",
        "agent-a",
        json!([message("shared-message")]),
    );
    let group = topic_frame(
        "shared-topic",
        "group",
        "group-a",
        json!([message("shared-message")]),
    );
    let agent_frame = parse_topic_ndjson_frame(&serde_json::to_vec(&agent).unwrap()).unwrap();
    let group_frame = parse_topic_ndjson_frame(&serde_json::to_vec(&group).unwrap()).unwrap();
    let expected = HashSet::from([agent_frame.topic.clone()]);
    validate_returned_topic_identity(&agent_frame, &expected).unwrap();
    assert!(validate_returned_topic_identity(&group_frame, &expected).is_err());

    let expected_ids = HashSet::from(["shared-message".to_string()]);
    validate_requested_message_ids(
        &agent_frame.topic,
        Some(&expected_ids),
        &agent_frame.messages,
    )
    .unwrap();

    let both_topics = HashSet::from([agent_frame.topic.clone(), group_frame.topic.clone()]);
    validate_returned_topic_identity(&agent_frame, &both_topics).unwrap();
    validate_returned_topic_identity(&group_frame, &both_topics).unwrap();
    assert_ne!(agent_frame.topic, group_frame.topic);
}

#[test]
fn per_topic_error_requires_exact_wire_error_and_has_no_live_payload() {
    let value = json!({
        "kind": "topic",
        "topicId": "topic",
        "ownerType": "agent",
        "ownerId": "agent-a",
        "ok": false,
        "error": {
            "code": "SYNC_MESSAGE_READ_FAILED",
            "origin": "desktop_plugin",
            "stage": "messages",
            "kind": "storage",
            "retry": "manual",
            "message": "read failed",
            "failedTopicIds": ["topic"],
        },
    });
    let frame = parse_topic_ndjson_frame(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(frame.error.is_some());
    assert!(parse_topic_ndjson_frame(
        &serde_json::to_vec(&json!({
            "kind": "topic", "topicId": "topic", "ownerType": "agent", "ownerId": "agent-a", "ok": false,
        }))
        .unwrap()
    )
    .is_err());
    assert!(parse_topic_ndjson_frame(
        &serde_json::to_vec(&json!({
            "kind": "topic", "topicId": "topic", "ownerType": "agent", "ownerId": "agent-a", "ok": true,
            "messages": [], "error": null,
        }))
        .unwrap()
    )
    .is_err());
}

#[test]
fn stream_error_uses_kind_stream_error_and_strict_error_contract() {
    let line = br#"{"kind":"streamError","error":{"code":"SYNC_STREAM_FAILED","origin":"desktop_plugin","stage":"messages","kind":"connection","retry":"manual","message":"stream failed","failedTopicIds":[]}}"#;
    assert!(parse_stream_error_frame(line).unwrap().is_some());
    let duplicate = br#"{"kind":"streamError","error":{"code":"SYNC_STREAM_FAILED","code":"SYNC_STREAM_FAILED","origin":"desktop_plugin","stage":"messages","kind":"connection","retry":"manual","message":"stream failed","failedTopicIds":[]}}"#;
    assert!(parse_stream_error_frame(duplicate).is_err());
    let old_marker = br#"{"_stream_error":{"code":"SYNC_STREAM_FAILED"}}"#;
    assert!(parse_stream_error_frame(old_marker).unwrap().is_none());
}

#[test]
fn ndjson_byte_split_preserves_utf8_and_enforces_budgets() {
    let bytes = serde_json::to_vec(&topic_frame("主题", "agent", "agent-a", json!([]))).unwrap();
    let newline = [b'\n'];
    let mut joined = bytes.clone();
    joined.extend_from_slice(&newline);
    let split = joined
        .windows("题".len())
        .position(|window| window == "题".as_bytes())
        .unwrap()
        + 1;
    let mut buffer = NdjsonLineBuffer::default();
    assert!(buffer.push(&joined[..split]).unwrap().is_empty());
    let lines = buffer.push(&joined[split..]).unwrap();
    assert_eq!(lines.len(), 1);
    let parsed = parse_topic_ndjson_frame(&lines[0][..lines[0].len() - 1]).unwrap();
    assert_eq!(parsed.topic.topic_id, "主题");

    assert!(buffer.push(&vec![b'a'; MAX_NDJSON_LINE_BYTES + 1]).is_err());
    let mut budget = super::ndjson_codec::NdjsonBudget::new(1);
    budget.observe_chunk(MAX_NDJSON_TOTAL_BYTES).unwrap();
    assert!(budget.observe_chunk(1).is_err());
    let mut budget = super::ndjson_codec::NdjsonBudget::new(1);
    assert!(budget.observe_frame(MAX_NDJSON_LINE_BYTES + 1, 0).is_err());
    let mut budget = super::ndjson_codec::NdjsonBudget::new(1);
    budget.observe_frame(1, MAX_NDJSON_ENTITIES).unwrap();
    assert!(budget.observe_frame(1, 1).is_err());
}

#[test]
fn avatar_validation_requires_allowlisted_mime_matching_magic_and_sha256() {
    let png = b"\x89PNG\r\n\x1a\nmock";
    let hash = validate_avatar_bytes(png, "image/png; charset=binary").unwrap();
    assert_eq!(hash.len(), 64);

    for (bytes, mime) in [
        (png.as_slice(), "image/jpeg"),
        (b"not-an-image".as_slice(), "image/png"),
        (png.as_slice(), "text/plain"),
    ] {
        assert!(validate_avatar_bytes(bytes, mime).is_err());
    }
    assert!(validate_avatar_bytes(&[], "image/png").is_err());
}

#[test]
fn progress_payload_echoes_session_and_attempt_identity() {
    let context = PullProgressContext {
        session_id: 11,
        attempt_id: 17,
        base_completed: 2,
        total: 10,
        failed: 1,
        legacy_attachment_warnings: 3,
    };
    let payload = progress_payload(&context, 4);
    assert_eq!(payload["sessionId"], 11);
    assert_eq!(payload["attemptId"], 17);
    assert_eq!(payload["completed"], 6);
}
