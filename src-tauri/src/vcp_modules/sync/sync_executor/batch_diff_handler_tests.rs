use super::*;
use serde_json::json;
use std::collections::HashSet;

#[test]
fn phase3_results_must_cover_the_exact_requested_topic_set() {
    let expected = HashSet::from(["topic-a".to_string(), "topic-b".to_string()]);
    let complete = json!({
        "topic-a": { "ok": true, "toPush": false, "toPull": [] },
        "topic-b": { "ok": true, "toPush": false, "toPull": [] }
    });
    assert!(validate_phase3_result_topics(
        &expected,
        complete.as_object().expect("complete result map"),
    )
    .is_ok());

    let incomplete = json!({
        "topic-a": { "ok": true, "toPush": false, "toPull": [] }
    });
    let error = validate_phase3_result_topics(
        &expected,
        incomplete.as_object().expect("incomplete result map"),
    )
    .expect_err("missing topic must fail phase 3");
    assert!(error.contains("topic-b"));

    let unexpected = json!({
        "topic-a": { "ok": true, "toPush": false, "toPull": [] },
        "topic-c": { "ok": true, "toPush": false, "toPull": [] }
    });
    let error = validate_phase3_result_topics(
        &expected,
        unexpected.as_object().expect("unexpected result map"),
    )
    .expect_err("unexpected topic must fail phase 3");
    assert!(error.contains("topic-b"));
    assert!(error.contains("topic-c"));
}

#[test]
fn phase3_decision_is_a_strict_discriminated_union() {
    assert_eq!(
        parse_topic_decision(
            "topic-a",
            &json!({ "ok": true, "toPull": ["message-a"], "toPush": false })
        )
        .expect("valid decision"),
        TopicDecision {
            to_pull: vec!["message-a".to_string()],
            to_push: false,
        }
    );

    for invalid in [
        json!({ "toPull": [], "toPush": false }),
        json!({ "ok": true, "toPull": "message-a", "toPush": false }),
        json!({ "ok": true, "toPull": [], "toPush": "false" }),
        json!({ "ok": true, "toPull": ["message-a", "message-a"], "toPush": false }),
        json!({ "ok": true, "toPull": [], "toPush": false, "error": { "code": "X" } }),
        json!({ "ok": false, "error": { "code": "DESKTOP_DB", "message": "failed" } }),
        json!({ "ok": false, "toPull": [], "toPush": false, "error": { "code": "DESKTOP_DB" } }),
    ] {
        assert!(parse_topic_decision("topic-a", &invalid).is_err());
    }

    let rejection = parse_topic_decision(
        "topic-a",
        &json!({
            "ok": false,
            "error": {
                "code": "SYNC_OWNER_CONFLICT",
                "origin": "desktop_cds",
                "stage": "messages",
                "kind": "data",
                "retry": "manual",
                "message": "failed",
                "failedTopicIds": ["topic-a"]
            }
        }),
    )
    .expect_err("desktop rejection must terminate phase 3");
    assert_eq!(rejection.code, "SYNC_OWNER_CONFLICT");
    assert_eq!(rejection.failed_topic_ids, vec!["topic-a"]);
    assert_eq!(
        crate::vcp_modules::sync_error::decode_wire_sync_error(&rejection.message)
            .expect("encoded root error")
            .origin,
        crate::vcp_modules::sync_error::SyncErrorOrigin::DesktopCds
    );
}

#[test]
fn raw_phase3_parser_rejects_duplicate_topic_keys() {
    let duplicate = r#"{
        "type":"SYNC_DIFF_RESULTS_BATCH",
        "results":{
            "topic-a":{"ok":true,"toPull":[],"toPush":false},
            "topic-a":{"ok":true,"toPull":[],"toPush":false}
        }
    }"#;
    let error = parse_phase3_batch_frame(duplicate)
        .expect_err("duplicate raw JSON topic keys must not be overwritten");
    assert_eq!(error.code, "PHASE3_FRAME_INVALID");
    assert!(error.message.contains("duplicate"));
    assert!(error.message.contains("topic-a"));
}

#[test]
fn raw_phase3_parser_rejects_unknown_fields_and_trailing_json() {
    let unknown = r#"{
        "type":"SYNC_DIFF_RESULTS_BATCH",
        "results":{},
        "legacy":true
    }"#;
    assert_eq!(
        parse_phase3_batch_frame(unknown)
            .expect_err("unknown frame field must fail closed")
            .code,
        "PHASE3_FRAME_INVALID"
    );

    let trailing = r#"{"type":"SYNC_DIFF_RESULTS_BATCH","results":{}} {"extra":true}"#;
    assert_eq!(
        parse_phase3_batch_frame(trailing)
            .expect_err("trailing JSON must fail closed")
            .code,
        "PHASE3_FRAME_INVALID"
    );
}

#[test]
fn raw_phase3_parser_rejects_nested_duplicate_keys() {
    let duplicate = r#"{
        "type":"SYNC_DIFF_RESULTS_BATCH",
        "results":{
            "topic-a":{"ok":true,"toPull":[],"toPush":false,"toPush":true}
        }
    }"#;
    assert!(parse_phase3_batch_frame(duplicate).is_err());
}

#[test]
fn push_topic_failure_rejects_the_batch() {
    let expected = vec!["topic-a".to_string()];
    let error = validate_topic_batch_outcomes(
        "push",
        &expected,
        Ok(vec![TopicBatchOutcome {
            topic_id: "topic-a".to_string(),
            success: false,
            error: Some("desktop rejected upload".to_string()),
        }]),
    )
    .expect_err("a false push result must fail phase 3");
    assert!(error.contains("topic-a"));
    assert!(error.contains("desktop rejected upload"));
}

#[test]
fn missing_pull_topic_rejects_the_batch() {
    let expected = vec!["topic-a".to_string(), "topic-b".to_string()];
    let error = validate_topic_batch_outcomes(
        "pull",
        &expected,
        Ok(vec![TopicBatchOutcome {
            topic_id: "topic-a".to_string(),
            success: true,
            error: None,
        }]),
    )
    .expect_err("a missing pull response must fail phase 3");
    assert!(error.contains("topic-b"));
    assert!(error.contains("missing from batch response"));
}

#[test]
fn duplicate_or_unexpected_batch_outcomes_are_rejected() {
    let expected = vec!["topic-a".to_string()];
    let duplicate = validate_topic_batch_outcomes(
        "push",
        &expected,
        Ok(vec![
            TopicBatchOutcome {
                topic_id: "topic-a".to_string(),
                success: true,
                error: None,
            },
            TopicBatchOutcome {
                topic_id: "topic-a".to_string(),
                success: true,
                error: None,
            },
        ]),
    )
    .expect_err("duplicate response topic must fail");
    assert!(duplicate.contains("duplicate topic"));

    let unexpected = validate_topic_batch_outcomes(
        "push",
        &expected,
        Ok(vec![TopicBatchOutcome {
            topic_id: "topic-b".to_string(),
            success: true,
            error: None,
        }]),
    )
    .expect_err("unexpected response topic must fail");
    assert!(unexpected.contains("unexpected topic topic-b"));
}

#[test]
fn batch_error_rejects_all_expected_topics() {
    let expected = vec!["topic-b".to_string(), "topic-a".to_string()];
    let error =
        validate_topic_batch_outcomes("push", &expected, Err("transport closed".to_string()))
            .expect_err("a batch-level error must fail phase 3");
    assert!(error.contains("topic-a"));
    assert!(error.contains("topic-b"));
    assert!(error.contains("transport closed"));
}
