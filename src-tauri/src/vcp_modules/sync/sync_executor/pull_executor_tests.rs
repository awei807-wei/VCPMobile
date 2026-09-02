use super::frame_validation::{
    parse_topic_ndjson_frame, validate_requested_message_ids, validate_returned_topic_identity,
};
use super::ndjson_codec::{parse_stream_error_frame, pull_worker_permits, NdjsonBudget};
use super::{MAX_NDJSON_ENTITIES, MAX_NDJSON_LINE_BYTES, MAX_NDJSON_TOTAL_BYTES};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const PROTOCOL_FIXTURE_SHA256: &str =
    "7226118ea55766f952575032efc8cfff883a19c9d196f637ac267cb8795fcef8";

fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("../fixtures/protocol_1_2_golden.json"))
        .expect("protocol fixture must be valid JSON")
}

fn message(id: &str) -> Value {
    json!({
        "id": id,
        "role": "user",
        "content": "hello",
        "timestamp": 1,
    })
}

fn frame_value(topic_id: Value, messages: Value) -> Value {
    json!({ "topicId": topic_id, "messages": messages })
}

fn frame(topic_id: &str, messages: Value) -> Value {
    frame_value(json!(topic_id), messages)
}

#[test]
fn protocol_fixture_is_byte_exact_and_valid_frame_is_accepted() {
    let bytes = include_bytes!("../fixtures/protocol_1_2_golden.json");
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        PROTOCOL_FIXTURE_SHA256
    );

    let input = &fixture()["validFrames"][0]["input"];
    let parsed = parse_topic_ndjson_frame(&serde_json::to_vec(input).unwrap())
        .expect("valid Wire 1.2 frame");
    assert_eq!(parsed.topic_id, "topic-golden");
    assert_eq!(parsed.messages.len(), 2);
    assert_eq!(parsed.legacy_attachment_warnings, 4);
    assert_eq!(parsed.messages[0].attachments.as_ref().unwrap().len(), 3);
}

#[test]
fn fixture_invalid_frames_match_fail_closed_validation() {
    for entry in fixture()["invalidFrames"].as_array().unwrap() {
        let input = serde_json::to_vec(&entry["input"]).unwrap();
        let error = parse_topic_ndjson_frame(&input).expect_err("invalid frame");
        assert!(error.contains(entry["errorContains"].as_str().unwrap()));
    }
}

#[test]
fn frame_topic_id_must_be_non_empty_string() {
    for topic_id in [json!(null), json!(""), json!(42)] {
        assert!(parse_topic_ndjson_frame(
            &serde_json::to_vec(&frame_value(topic_id, json!([]))).unwrap()
        )
        .is_err());
    }
}

#[test]
fn messages_must_be_array_and_ids_must_be_non_empty_and_unique() {
    for messages in [json!(null), json!({}), json!("messages")] {
        assert!(parse_topic_ndjson_frame(
            &serde_json::to_vec(&frame_value(json!("t"), messages)).unwrap()
        )
        .is_err());
    }
    for messages in [
        json!([{}]),
        json!([{ "id": "" }]),
        json!([{ "id": 7 }]),
        json!([{ "id": "same" }, { "id": "same" }]),
    ] {
        assert!(parse_topic_ndjson_frame(
            &serde_json::to_vec(&frame_value(json!("t"), messages)).unwrap()
        )
        .is_err());
    }
}

#[test]
fn message_topic_id_missing_or_null_is_allowed_when_present_exactly_matches() {
    for topic_id in [None, Some(Value::Null), Some(json!("topic"))] {
        let mut item = message("m");
        if let Some(topic_id) = topic_id {
            item["topicId"] = topic_id;
        }
        let parsed =
            parse_topic_ndjson_frame(&serde_json::to_vec(&frame("topic", json!([item]))).unwrap())
                .expect("missing/null/matching message topicId is valid");
        assert_eq!(parsed.messages.len(), 1);
    }
}

#[test]
fn message_topic_id_conflict_or_non_string_is_rejected_without_normalization() {
    for topic_id in [json!("other"), json!(7), json!({ "nested": true })] {
        let mut item = message("m");
        item["topicId"] = topic_id;
        let error =
            parse_topic_ndjson_frame(&serde_json::to_vec(&frame("topic", json!([item]))).unwrap())
                .expect_err("conflicting or non-string message topicId");
        assert!(error.contains("topicId"));
    }
}

#[test]
fn duplicate_json_keys_are_rejected_before_deserialization() {
    let duplicate = br#"{"topicId":"topic","topicId":"topic","messages":[]}"#;
    let error = parse_topic_ndjson_frame(duplicate).expect_err("duplicate key");
    assert!(error.contains("duplicate JSON object key"));
}

#[test]
fn tombstone_messages_are_rejected_as_live_frames() {
    for tombstone in [json!({ "status": "removed" }), json!({ "deletedAt": 2 })] {
        let mut item = message("m");
        if let Some(object) = tombstone.as_object() {
            for (key, value) in object {
                item[key] = value.clone();
            }
        }
        let error =
            parse_topic_ndjson_frame(&serde_json::to_vec(&frame("topic", json!([item]))).unwrap())
                .expect_err("tombstone in live frame");
        assert!(error.contains("Tombstoned message"));
    }
}

#[test]
fn returned_owner_identity_must_match_request_exactly() {
    let input = json!({
        "topicId": "topic",
        "ownerType": "agent",
        "ownerId": "agent-a",
        "messages": [],
    });
    let parsed = parse_topic_ndjson_frame(&serde_json::to_vec(&input).unwrap()).unwrap();
    let mut expected = HashMap::new();
    expected.insert(
        "topic".to_string(),
        ("agent".to_string(), "agent-a".to_string()),
    );
    validate_returned_topic_identity(&parsed, &expected).unwrap();

    for (owner_type, owner_id) in [("group", "agent-a"), ("agent", "agent-b")] {
        let input = json!({
            "topicId": "topic",
            "ownerType": owner_type,
            "ownerId": owner_id,
            "messages": [],
        });
        let parsed = parse_topic_ndjson_frame(&serde_json::to_vec(&input).unwrap()).unwrap();
        assert!(validate_returned_topic_identity(&parsed, &expected).is_err());
    }
}

#[test]
fn unknown_frame_fields_and_invalid_error_envelopes_fail_closed() {
    let unknown = json!({
        "topicId": "topic",
        "messages": [],
        "surprise": true,
    });
    assert!(parse_topic_ndjson_frame(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let error = json!({
        "topicId": "topic",
        "_error": {
            "code": "SYNC_DB_QUERY_FAILED",
            "origin": "desktop_plugin",
            "stage": "messages",
            "kind": "storage",
            "retry": "manual",
            "message": "query failed",
            "failedTopicIds": [],
            "debug": "unknown",
        },
    });
    assert!(parse_topic_ndjson_frame(&serde_json::to_vec(&error).unwrap()).is_err());
}

#[test]
fn valid_error_frame_can_omit_live_messages() {
    let error = json!({
        "topicId": "topic",
        "_error": {
            "code": "SYNC_DB_QUERY_FAILED",
            "origin": "desktop_plugin",
            "stage": "messages",
            "kind": "storage",
            "retry": "manual",
            "message": "query failed",
            "failedTopicIds": [],
        },
    });
    let parsed = parse_topic_ndjson_frame(&serde_json::to_vec(&error).unwrap()).unwrap();
    assert!(parsed.error.is_some());
    assert!(parsed.messages.is_empty());
}

#[test]
fn stream_error_uses_strict_error_contract() {
    let line = br#"{"_stream_error":{"code":"SYNC_DB_QUERY_FAILED","origin":"desktop_plugin","stage":"messages","kind":"storage","retry":"manual","message":"query failed","failedTopicIds":[]}}"#;
    assert!(parse_stream_error_frame(line).unwrap().is_some());
    let duplicate = br#"{"_stream_error":{"code":"SYNC_DB_QUERY_FAILED","code":"SYNC_DB_QUERY_FAILED","origin":"desktop_plugin","stage":"messages","kind":"storage","retry":"manual","message":"query failed","failedTopicIds":[]}}"#;
    assert!(parse_stream_error_frame(duplicate).is_err());
}

#[test]
fn requested_message_ids_must_match_returned_set() {
    let expected = HashSet::from(["m1".to_string(), "m2".to_string()]);
    let parsed = parse_topic_ndjson_frame(
        &serde_json::to_vec(&frame("topic", json!([message("m1"), message("m2")]))).unwrap(),
    )
    .unwrap();
    validate_requested_message_ids("topic", Some(&expected), &parsed.messages).unwrap();
    let wrong = HashSet::from(["m1".to_string()]);
    assert!(validate_requested_message_ids("topic", Some(&wrong), &parsed.messages).is_err());
}

#[test]
fn ndjson_budgets_remain_bounded() {
    let mut budget = NdjsonBudget::new(1);
    budget.observe_chunk(MAX_NDJSON_TOTAL_BYTES).unwrap();
    assert!(budget.observe_chunk(1).is_err());

    let mut budget = NdjsonBudget::new(1);
    assert!(budget.observe_frame(MAX_NDJSON_LINE_BYTES + 1, 0).is_err());
    let mut budget = NdjsonBudget::new(1);
    budget.observe_frame(1, MAX_NDJSON_ENTITIES).unwrap();
    assert!(budget.observe_frame(1, 1).is_err());
}

#[test]
fn worker_permits_scale_with_frame_bytes() {
    assert_eq!(pull_worker_permits(0).unwrap(), 1);
    assert_eq!(pull_worker_permits(1024 * 1024).unwrap(), 1);
    assert_eq!(pull_worker_permits(1024 * 1024 + 1).unwrap(), 2);
}
