use super::diff_item_validation::OwnerIdentity;
use super::topic_push::{validate_topic_owner, TopicPushRequest};
use super::{
    consume_manifest_response_type, next_manifest_command, parse_delete_timestamp,
    validate_and_filter_diff_items, validate_diff_frame,
};
use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Mutex;

#[test]
fn default_topic_actions_are_exempt_but_still_have_valid_wire_shape() {
    let items = json!([
        {"id": "default", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
        {"id": "default", "action": "PULL", "ownerType": "agent", "ownerId": "agent-b"},
        {"id": "default", "action": "PUSH_DELETE", "deletedAt": 7, "ownerType": "group", "ownerId": "group-a"},
        {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
    ]);
    let (filtered, exempt) =
        validate_and_filter_diff_items(items.as_array().expect("array"), &SyncDataType::Topic)
            .expect("default topics are exempt, not duplicate-rejected");
    assert_eq!(exempt, 3);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["id"], "topic-1");
}

#[test]
fn default_topic_does_not_exempt_unknown_action_or_bad_delete_timestamp() {
    for item in [
        json!({"id": "default", "action": "UNKNOWN"}),
        json!({"id": "default", "action": "DELETE", "deletedAt": -1}),
    ] {
        assert!(validate_and_filter_diff_items(&[item], &SyncDataType::Topic).is_err());
    }
}

#[test]
fn duplicate_non_default_topic_identity_is_rejected() {
    let items = json!([
        {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
        {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
    ]);
    let error =
        validate_and_filter_diff_items(items.as_array().expect("array"), &SyncDataType::Topic)
            .expect_err("duplicate non-default ids must fail");
    assert!(error.contains("duplicate id topic-1"));
}

#[test]
fn same_topic_id_for_distinct_owners_fails_closed_before_storage_collision() {
    let items = json!([
        {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-a"},
        {"id": "topic-1", "action": "PULL", "ownerType": "agent", "ownerId": "agent-b"},
    ]);
    let error =
        validate_and_filter_diff_items(items.as_array().expect("array"), &SyncDataType::Topic)
            .expect_err("one-column topic storage cannot represent colliding owner identities");
    assert!(error.contains("duplicate id topic-1"));
}

#[test]
fn default_id_is_not_exempt_for_non_topic_types() {
    let items = json!([
        {"id": "default", "action": "PULL"},
        {"id": "default", "action": "PULL"},
    ]);
    assert!(
        validate_and_filter_diff_items(items.as_array().expect("array"), &SyncDataType::Agent)
            .is_err()
    );
}

#[test]
fn manifest_diff_frame_and_items_reject_unknown_or_incoherent_fields() {
    let exact = json!({
        "type": "SYNC_DIFF_RESULTS",
        "data": [{"id":"agent-a","action":"SKIP","mismatchedContent":true}],
        "dataType": "agent",
        "phase": 1
    });
    validate_diff_frame(&exact, &SyncDataType::Agent).expect("exact response frame");
    validate_and_filter_diff_items(
        exact["data"].as_array().expect("array"),
        &SyncDataType::Agent,
    )
    .expect("exact response item");
    let central = json!({
        "type": "SYNC_DIFF_RESULTS",
        "data": [],
        "dataType": "agent"
    });
    validate_diff_frame(&central, &SyncDataType::Agent).expect("exact CDS response frame");
    let expected = Mutex::new(HashSet::from(["agent".to_string()]));
    assert!(
        consume_manifest_response_type(&central, &SyncDataType::Agent, 1, &expected,)
            .expect("CDS response infers the already registered manifest wave")
    );

    for payload in [
        json!({"type":"SYNC_DIFF_RESULTS","data":[],"dataType":"agent","phase":1,"debug":true}),
        json!({"type":"SYNC_DIFF_RESULTS","data":[],"dataType":"group","phase":1}),
    ] {
        assert!(validate_diff_frame(&payload, &SyncDataType::Agent).is_err());
    }
    for item in [
        json!({"id":"agent-a","action":"PULL","debug":true}),
        json!({"id":"agent-a","action":"PULL","deletedAt":1}),
    ] {
        assert!(validate_and_filter_diff_items(&[item], &SyncDataType::Agent).is_err());
    }
    validate_and_filter_diff_items(
        &[json!({"id":"topic-a","action":"PULL","ownerType":"agent","ownerId":"a","mismatchedContent":true})],
        &SyncDataType::Topic,
    )
    .expect("CDS may report a topic content mismatch with a metadata action");
    assert!(validate_and_filter_diff_items(
        &[json!({"id":"agent:a","action":"PULL","mismatchedContent":true})],
        &SyncDataType::Avatar,
    )
    .is_err());
}

#[test]
fn manifest_responses_consume_exact_type_once_for_current_phase() {
    let expected = Mutex::new(HashSet::from(["agent".to_string(), "group".to_string()]));
    assert!(!consume_manifest_response_type(
        &json!({"phase": 1}),
        &SyncDataType::Agent,
        1,
        &expected,
    )
    .expect("first expected type"));
    assert!(consume_manifest_response_type(
        &json!({"phase": 1}),
        &SyncDataType::Agent,
        1,
        &expected,
    )
    .is_err());
    assert!(
        consume_manifest_response_type(&json!({}), &SyncDataType::Group, 1, &expected,).is_err()
    );
    assert!(consume_manifest_response_type(
        &json!({"phase": 2}),
        &SyncDataType::Group,
        1,
        &expected,
    )
    .is_err());
    assert!(consume_manifest_response_type(
        &json!({"phase": 1}),
        &SyncDataType::Group,
        1,
        &expected,
    )
    .expect("last expected type"));
}

#[test]
fn avatar_wave_uses_owner_wire_phase_and_precedes_topics() {
    let expected = Mutex::new(HashSet::from(["avatar".to_string()]));
    assert!(consume_manifest_response_type(
        &json!({"phase": 1}),
        &SyncDataType::Avatar,
        2,
        &expected,
    )
    .expect("avatar response"));
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
fn delete_actions_require_stable_non_negative_timestamp() {
    assert_eq!(
        parse_delete_timestamp(
            &json!({"action": "DELETE", "deletedAt": 42}),
            "entity",
            "DELETE",
        )
        .expect("valid timestamp"),
        Some(42)
    );
    for value in [
        json!({"action": "DELETE"}),
        json!({"action": "DELETE", "deletedAt": null}),
        json!({"action": "DELETE", "deletedAt": "42"}),
        json!({"action": "DELETE", "deletedAt": -1}),
    ] {
        assert!(parse_delete_timestamp(&value, "entity", "DELETE").is_err());
    }
    assert_eq!(
        parse_delete_timestamp(&json!({"action": "SKIP"}), "entity", "SKIP")
            .expect("non-delete action"),
        None
    );
}

#[test]
fn topic_owner_identity_is_required_and_typed() {
    for item in [
        json!({"id": "topic-a", "action": "PULL"}),
        json!({"id": "topic-a", "action": "PUSH", "ownerType": "agent"}),
        json!({"id": "topic-a", "action": "PULL", "ownerType": "user", "ownerId": "x"}),
        json!({"id": "topic-a", "action": "PULL", "ownerType": "agent", "ownerId": 7}),
    ] {
        assert!(validate_and_filter_diff_items(&[item], &SyncDataType::Topic).is_err());
    }
}

#[test]
fn topic_push_rejects_database_owner_mismatch() {
    let request = TopicPushRequest {
        id: "topic-a".to_string(),
        owner: OwnerIdentity {
            owner_type: "agent".to_string(),
            owner_id: "agent-a".to_string(),
        },
    };
    assert!(validate_topic_owner(&request, "group", "group-a").is_err());
    assert!(validate_topic_owner(&request, "agent", "agent-a").is_ok());
}

#[test]
fn message_delete_requires_topic_identity() {
    assert!(validate_and_filter_diff_items(
        &[json!({"id": "message-a", "action": "DELETE", "deletedAt": 7})],
        &SyncDataType::Message,
    )
    .is_err());
    assert!(validate_and_filter_diff_items(
        &[json!({"id": "message-a", "action": "DELETE", "deletedAt": 7, "topicId": "topic-a"})],
        &SyncDataType::Message,
    )
    .is_ok());
}
