use super::manifest::ManifestDecision;
use super::topic_push::{validate_topic_owner, TopicPushRequest};
use super::{consume_manifest_response_type, next_manifest_command, validate_manifest_result};
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::{ManifestResultFrame, ManifestType};
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Mutex;

fn owner_result(action: &str) -> ManifestResultFrame {
    serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": action
        }]
    }))
    .expect("owner result frame")
}

fn topic_result(items: serde_json::Value) -> ManifestResultFrame {
    serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "topic",
        "results": items
    }))
    .expect("topic result frame")
}

#[test]
fn typed_topic_results_keep_full_identity_and_reject_duplicates() {
    let result = topic_result(json!([
        {
            "ownerType": "agent",
            "ownerId": "agent-a",
            "topicId": "shared",
            "action": "PULL"
        },
        {
            "ownerType": "group",
            "ownerId": "group-a",
            "topicId": "shared",
            "action": "PUSH"
        }
    ]));
    let (kind, decisions) = validate_manifest_result(result).expect("distinct identities");
    assert_eq!(kind, ManifestType::Topic);
    assert_eq!(decisions.len(), 2);
    assert!(matches!(&decisions[0], ManifestDecision::Topic(item) if item.topic_id == "shared"));

    let duplicate = serde_json::from_value::<ManifestResultFrame>(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "topic",
        "results": [
            {
                "ownerType": "agent",
                "ownerId": "agent-a",
                "topicId": "shared",
                "action": "PULL"
            },
            {
                "ownerType": "agent",
                "ownerId": "agent-a",
                "topicId": "shared",
                "action": "PUSH"
            }
        ]
    }));
    assert!(duplicate.is_err());
}

#[test]
fn manifest_types_are_consumed_once_and_wave_commands_keep_order() {
    let expected = Mutex::new(HashSet::from([ManifestType::Owner, ManifestType::Avatar]));
    assert!(
        !consume_manifest_response_type(ManifestType::Owner, 1, &expected,)
            .expect("first manifest type")
    );
    assert!(consume_manifest_response_type(ManifestType::Owner, 1, &expected,).is_err());
    assert!(consume_manifest_response_type(ManifestType::Topic, 1, &expected,).is_err());
    assert!(
        consume_manifest_response_type(ManifestType::Avatar, 1, &expected,)
            .expect("last manifest type")
    );
    assert!(matches!(
        next_manifest_command(1, 7),
        Some(SyncCommand::StartAvatarMetadata { attempt_id: 7 })
    ));
    assert!(matches!(
        next_manifest_command(2, 7),
        Some(SyncCommand::StartTopicMetadata { attempt_id: 7 })
    ));
    assert!(matches!(
        next_manifest_command(3, 7),
        Some(SyncCommand::StartTopicValidation { attempt_id: 7 })
    ));
}

#[test]
fn manifest_result_rejects_unknown_fields_and_invalid_frame_type() {
    assert!(serde_json::from_value::<ManifestResultFrame>(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": "PUSH",
            "debug": true
        }]
    }))
    .is_err());
    assert!(serde_json::from_value::<ManifestResultFrame>(json!({
        "type": "SYNC_DIFF_RESULTS",
        "manifestType": "owner",
        "results": []
    }))
    .is_err());
}

#[test]
fn all_five_actions_are_typed_and_delete_actions_require_tombstones() {
    for (action, deleted_at) in [
        ("PULL", None),
        ("PUSH", None),
        ("PULL_DELETE", Some(42)),
        ("PUSH_DELETE", Some(43)),
        ("SKIP", None),
    ] {
        let mut result = json!([{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": action
        }]);
        if let Some(deleted_at) = deleted_at {
            result[0]["deletedAt"] = json!(deleted_at);
        }
        let frame: ManifestResultFrame = serde_json::from_value(json!({
            "type": "SYNC_MANIFEST_RESULT",
            "manifestType": "owner",
            "results": result
        }))
        .expect("known action result frame");
        if action == "SKIP" {
            assert!(validate_manifest_result(frame).is_err());
        } else {
            // Owner SKIP needs contentHashMismatch and is checked below.
            assert!(validate_manifest_result(frame).is_ok());
        }
    }
    for deleted_at in [None::<i64>, Some(-1_i64), Some(9_007_199_254_740_992_i64)] {
        let mut result = json!([{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": "PULL_DELETE"
        }]);
        if let Some(value) = deleted_at {
            result[0]["deletedAt"] = json!(value);
        }
        let frame: Result<ManifestResultFrame, _> = serde_json::from_value(json!({
            "type": "SYNC_MANIFEST_RESULT",
            "manifestType": "owner",
            "results": result
        }));
        assert!(frame.is_err(), "invalid deletedAt {deleted_at:?} must fail");
    }
}

#[test]
fn owner_skip_requires_content_mismatch_and_delete_cannot_claim_mismatch() {
    let without_mismatch = owner_result("SKIP");
    assert!(validate_manifest_result(without_mismatch).is_err());

    let with_mismatch: ManifestResultFrame = serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": "SKIP",
            "contentHashMismatch": true
        }]
    }))
    .expect("owner mismatch decision");
    assert!(validate_manifest_result(with_mismatch).is_ok());

    let invalid_delete: ManifestResultFrame = serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": "PULL_DELETE",
            "deletedAt": 1,
            "contentHashMismatch": true
        }]
    }))
    .expect("delete decision frame");
    assert!(validate_manifest_result(invalid_delete).is_err());
}

#[test]
fn topic_and_avatar_skip_decisions_are_rejected() {
    let topic = topic_result(json!([{
        "ownerType": "agent",
        "ownerId": "agent-a",
        "topicId": "topic-a",
        "action": "SKIP"
    }]));
    assert!(validate_manifest_result(topic).is_err());

    let avatar: ManifestResultFrame = serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "avatar",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "action": "SKIP"
        }]
    }))
    .expect("avatar decision frame");
    assert!(validate_manifest_result(avatar).is_err());
}

#[test]
fn topic_push_request_requires_database_owner_and_topic_to_match() {
    let request = TopicPushRequest::new(TopicKey::new("agent", "agent-a", "topic-a"));
    assert!(validate_topic_owner(&request.key, "group", "group-a", "topic-a").is_err());
    assert!(validate_topic_owner(&request.key, "agent", "agent-a", "other").is_err());
    assert!(validate_topic_owner(&request.key, "agent", "agent-a", "topic-a").is_ok());
}
