use super::{
    parse_message_diff_result_frame, parse_topic_decision, validate_phase3_result_topics,
    validate_topic_batch_outcomes, TopicBatchOutcome, TopicDecision,
};
use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, MessageDiffDecision, OwnerType};
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;
use std::collections::HashSet;

fn topic(owner_type: &str, owner_id: &str, topic_id: &str) -> TopicKey {
    TopicKey::new(owner_type, owner_id, topic_id)
}

fn decision(key: &TopicKey) -> MessageDiffDecision {
    MessageDiffDecision {
        owner_type: if key.owner_type == "group" {
            OwnerType::Group
        } else {
            OwnerType::Agent
        },
        owner_id: key.owner_id.clone(),
        topic_id: key.topic_id.clone(),
        ok: true,
        pull_message_ids: Some(Vec::new()),
        push_topic: Some(false),
        delete_messages: Some(Vec::new()),
        error: None,
    }
}

#[test]
fn result_coverage_uses_the_complete_topic_identity() {
    let agent = topic("agent", "shared-owner-id", "shared-topic-id");
    let group = topic("group", "shared-owner-id", "shared-topic-id");
    let expected = HashSet::from([agent.clone(), group.clone()]);
    let results = vec![decision(&agent), decision(&group)];
    assert!(validate_phase3_result_topics(&expected, &results).is_ok());

    let only_agent = vec![decision(&agent)];
    let error = validate_phase3_result_topics(&expected, &only_agent)
        .expect_err("same topic id in another owner namespace must not satisfy coverage");
    assert!(error.contains("group"));
}

#[test]
fn decision_is_a_strict_live_or_tombstone_union() {
    let key = topic("agent", "agent-a", "topic-a");
    let mut valid = decision(&key);
    valid.pull_message_ids = Some(vec!["message-a".to_string()]);
    assert_eq!(
        parse_topic_decision(&key, &valid).expect("valid decision"),
        TopicDecision {
            pull_message_ids: vec!["message-a".to_string()],
            push_topic: false,
            delete_messages: Vec::new(),
        }
    );

    let mut deleted = decision(&key);
    deleted.delete_messages = Some(vec![MessageDeleteDecision {
        msg_id: "message-b".to_string(),
        deleted_at: 1234,
    }]);
    assert_eq!(
        parse_topic_decision(&key, &deleted)
            .expect("valid tombstone decision")
            .delete_messages,
        vec![MessageDeleteDecision {
            msg_id: "message-b".to_string(),
            deleted_at: 1234,
        }]
    );

    let mut invalid = decision(&key);
    invalid.pull_message_ids = None;
    assert!(parse_topic_decision(&key, &invalid).is_err());

    let mut overlap = decision(&key);
    overlap.pull_message_ids = Some(vec!["same".to_string()]);
    overlap.delete_messages = Some(vec![MessageDeleteDecision {
        msg_id: "same".to_string(),
        deleted_at: 1,
    }]);
    assert!(parse_topic_decision(&key, &overlap).is_err());
}

#[test]
fn strict_result_parser_rejects_wire12_and_duplicate_or_unknown_fields() {
    let legacy = r#"{
        "type":"SYNC_DIFF_RESULTS_BATCH",
        "results":{}
    }"#;
    assert_eq!(
        parse_message_diff_result_frame(legacy)
            .expect_err("Wire 1.2 result maps must not be accepted")
            .code,
        "PHASE3_FRAME_INVALID"
    );

    let unknown = r#"{
        "type":"SYNC_MESSAGE_DIFF_RESULT",
        "results":[],
        "legacy":true
    }"#;
    assert_eq!(
        parse_message_diff_result_frame(unknown)
            .expect_err("unknown frame fields must fail closed")
            .code,
        "PHASE3_FRAME_INVALID"
    );

    let duplicate = r#"{
        "type":"SYNC_MESSAGE_DIFF_RESULT",
        "type":"SYNC_MESSAGE_DIFF_RESULT",
        "results":[]
    }"#;
    assert_eq!(
        parse_message_diff_result_frame(duplicate)
            .expect_err("duplicate frame fields must fail closed")
            .code,
        "PHASE3_FRAME_INVALID"
    );
}

#[test]
fn batch_outcomes_fail_closed_and_keep_restartable_error_codes() {
    let a = topic("agent", "agent-a", "topic-a");
    let b = topic("group", "group-a", "topic-b");
    let success = |topic: TopicKey| TopicBatchOutcome {
        topic,
        success: true,
        error: None,
    };
    let failed = validate_topic_batch_outcomes(
        "pull",
        &[a.clone(), b.clone()],
        Ok(vec![success(a.clone())]),
    )
    .expect_err("missing topic must stop the batch");
    assert!(failed.message.contains("topic-b"));
    assert!(failed.restart_code.is_none());

    let stale = encode_local_sync_error(
        "SYNC_SNAPSHOT_STALE",
        SyncErrorStage::Messages,
        "history changed",
        vec![a.topic_id.clone()],
    );
    let restartable = validate_topic_batch_outcomes(
        "pull",
        std::slice::from_ref(&a),
        Ok(vec![TopicBatchOutcome {
            topic: a.clone(),
            success: false,
            error: Some(stale),
        }]),
    )
    .expect_err("stale snapshots still fail this batch");
    assert_eq!(
        restartable.restart_code.as_deref(),
        Some("SYNC_SNAPSHOT_STALE")
    );

    let duplicate = validate_topic_batch_outcomes(
        "push",
        std::slice::from_ref(&a),
        Ok(vec![success(a.clone()), success(a.clone())]),
    )
    .expect_err("duplicate topic outcomes must fail closed");
    assert!(duplicate.message.contains("duplicate"));
}

#[test]
fn typed_result_shape_requires_all_success_fields() {
    let value = json!({
        "type": "SYNC_MESSAGE_DIFF_RESULT",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "topicId": "topic-a",
            "ok": true,
            "pullMessageIds": [],
            "pushTopic": false
        }]
    });
    let frame = parse_message_diff_result_frame(&value.to_string()).expect("typed frame parses");
    let key = topic("agent", "agent-a", "topic-a");
    assert!(parse_topic_decision(&key, &frame.results[0]).is_err());
}
