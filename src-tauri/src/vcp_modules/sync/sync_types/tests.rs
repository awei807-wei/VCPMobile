use super::*;
use serde_json::json;

use super::*;

#[test]
fn canonical_json_escapes_dynamic_keys_and_is_injective_for_member_tags() {
    let injected = serde_json::json!({
        "memberTags": { "a\":\"x\",\"b": "y" }
    });
    let ordinary = serde_json::json!({
        "memberTags": { "a": "x", "b": "y" }
    });

    assert_eq!(
        stable_stringify(&injected),
        r#"{"memberTags":{"a\":\"x\",\"b":"y"}}"#
    );
    assert_ne!(stable_stringify(&injected), stable_stringify(&ordinary));
    assert_ne!(
        compute_deterministic_hash(&injected),
        compute_deterministic_hash(&ordinary)
    );
}

#[test]
fn canonical_json_sorts_keys_by_utf8_bytes_and_streams_the_same_digest() {
    let value = serde_json::json!({
        "memberTags": {
            "😀": "astral",
            "\u{e000}": "private-use",
            "line\nkey": "control",
            "slash\\key": "slash"
        }
    });
    let canonical = stable_stringify(&value);

    assert!(canonical.find('\u{e000}').unwrap() < canonical.find('😀').unwrap());
    assert!(canonical.contains(r#""line\nkey""#));
    assert!(canonical.contains(r#""slash\\key""#));
    assert_eq!(
        compute_deterministic_hash(&value),
        crate::vcp_modules::infra::utils::calculate_sha256(canonical.as_bytes())
    );
}

#[test]
fn canonical_hash_matches_the_shared_cross_language_vectors() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/message_canonical_contract.json"))
            .expect("parse canonical hash fixture");
    for case in fixture["canonicalHashCases"]
        .as_array()
        .expect("canonical hash cases")
    {
        let value = &case["value"];
        assert_eq!(
            stable_stringify(value),
            case["expectedCanonical"].as_str().expect("canonical bytes"),
            "case {}",
            case["name"].as_str().unwrap_or("unnamed")
        );
        assert_eq!(
            compute_deterministic_hash(value),
            case["expectedHash"].as_str().expect("canonical hash"),
            "case {}",
            case["name"].as_str().unwrap_or("unnamed")
        );
    }
}

#[test]
fn avatar_owner_contract_is_closed_and_keeps_the_user_singleton() {
    assert!(is_valid_avatar_owner("agent", "agent-1"));
    assert!(is_valid_avatar_owner("group", "group-1"));
    assert!(is_valid_avatar_owner("user", "user_avatar"));
    assert!(!is_valid_avatar_owner("user", "other"));
    assert!(!is_valid_avatar_owner("agent", ""));
    assert!(!is_valid_avatar_owner("system", "system"));
}

fn hash(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

#[test]
fn manifest_wire_uses_compound_identity_and_explicit_tombstones() {
    let request: ManifestRequest = serde_json::from_value(json!({
        "manifestType": "topic",
        "items": [
            {"ownerType": "agent", "ownerId": "agent-a", "topicId": "shared", "deletedAt": 10},
            {"ownerType": "group", "ownerId": "group-a", "topicId": "shared", "deletedAt": 11}
        ],
        "targetedOwners": [
            {"ownerType": "agent", "ownerId": "agent-a"},
            {"ownerType": "group", "ownerId": "group-a"}
        ]
    }))
    .expect("compound topic manifest should decode");

    request
        .validate()
        .expect("manifest identity should validate");
    let encoded = serde_json::to_value(&request).expect("manifest should encode");
    assert_eq!(encoded["manifestType"], "topic");
    assert!(encoded["items"][0].get("deletedAt").is_some());
    assert!(encoded["items"][0].get("configHash").is_none());
    assert!(encoded["items"][0].get("contentHash").is_none());
    assert_eq!(encoded["items"][0]["topicId"], "shared");
    assert!(serde_json::from_value::<ManifestRequest>(json!({
        "manifestType": "topic",
        "items": [],
        "targetedOwners": [],
        "legacyDataType": "topic"
    }))
    .is_err());
    assert!(serde_json::from_value::<ManifestRequest>(json!({
        "dataType": "topic",
        "data": []
    }))
    .is_err());
}

#[test]
fn manifest_action_enum_accepts_only_the_five_wire_actions() {
    let actions = [
        ("PULL", ManifestAction::Pull),
        ("PUSH", ManifestAction::Push),
        ("PULL_DELETE", ManifestAction::PullDelete),
        ("PUSH_DELETE", ManifestAction::PushDelete),
        ("SKIP", ManifestAction::Skip),
    ];
    for (wire, expected) in actions {
        assert_eq!(
            serde_json::from_str::<ManifestAction>(&format!("\"{wire}\""))
                .expect("known manifest action"),
            expected
        );
    }
    assert!(serde_json::from_str::<ManifestAction>("\"DELETE\"").is_err());
}

#[test]
fn manifest_result_rejects_unknown_fields_and_invalid_frame_type() {
    let result: ManifestResultFrame = serde_json::from_value(json!({
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{"ownerType": "agent", "ownerId": "agent-a", "action": "PUSH"}]
    }))
    .expect("strict manifest result should decode");
    assert_eq!(result.manifest_type(), ManifestType::Owner);
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
        "type": "SYNC_MANIFEST_RESULT",
        "manifestType": "owner",
        "results": [{"ownerType": "agent", "ownerId": "agent-a", "action": "DELETE"}]
    }))
    .is_err());
}

#[test]
fn topic_diff_keeps_owner_identity_in_requests_and_results() {
    let state = TopicDiffState {
        owner_type: OwnerType::Agent,
        owner_id: "agent-a".to_string(),
        topic_id: "shared".to_string(),
        config_hash: hash('a'),
        content_hash: String::new(),
    };
    let request = TopicDiffRequestFrame::new(vec![state]);
    request.validate().expect("topic diff should validate");
    let encoded = serde_json::to_value(request).expect("topic diff should encode");
    assert_eq!(encoded["type"], "SYNC_TOPIC_DIFF_REQUEST");
    assert_eq!(encoded["topics"][0]["ownerId"], "agent-a");
    assert_eq!(encoded["topics"][0]["topicId"], "shared");

    let result: TopicDiffResultFrame = serde_json::from_value(json!({
        "type": "SYNC_TOPIC_DIFF_RESULT",
        "changedTopics": [
            {"ownerType": "agent", "ownerId": "agent-a", "topicId": "shared"},
            {"ownerType": "group", "ownerId": "group-a", "topicId": "shared"}
        ]
    }))
    .expect("compound topic result should decode");
    assert_eq!(result.changed_topics.len(), 2);
    assert!(serde_json::from_value::<TopicDiffResultFrame>(json!({
        "type": "SYNC_TOPIC_DIFF_RESULT",
        "changedTopics": [{"ownerType": "agent", "ownerId": "agent-a", "topicId": "shared", "extra": 1}]
    }))
    .is_err());
}

#[test]
fn message_diff_supports_live_and_tombstone_states() {
    let state: MessageDiffTopicState = serde_json::from_value(json!({
        "ownerType": "agent",
        "ownerId": "agent-a",
        "topicId": "shared",
        "contentHash": "",
        "messages": {
            "live": {"messageHash": hash('b'), "updatedAt": 20},
            "deleted": {"deletedAt": 21}
        }
    }))
    .expect("message diff state should decode");
    assert!(matches!(
        state.messages.get("live"),
        Some(MessageVersionState::Live(_))
    ));
    assert!(matches!(
        state.messages.get("deleted"),
        Some(MessageVersionState::Deleted(_))
    ));
    let request = MessageDiffRequestFrame::new(vec![state]);
    request.validate().expect("message diff should validate");
    assert!(serde_json::from_value::<MessageDiffTopicState>(json!({
        "ownerType": "agent",
        "ownerId": "agent-a",
        "topicId": "shared",
        "contentHash": "",
        "messages": {"broken": {"messageHash": hash('b'), "updatedAt": 20, "deletedAt": 21}}
    }))
    .is_err());

    let result: MessageDiffResultFrame = serde_json::from_value(json!({
        "type": "SYNC_MESSAGE_DIFF_RESULT",
        "results": [{
            "ownerType": "agent",
            "ownerId": "agent-a",
            "topicId": "shared",
            "ok": true,
            "pullMessageIds": ["m1"],
            "pushTopic": false,
            "deleteMessages": [{"msgId": "m2", "deletedAt": 22}]
        }]
    }))
    .expect("message diff result should decode");
    assert_eq!(result.results.len(), 1);
}

#[test]
fn entity_pull_and_message_pull_reject_incomplete_identity() {
    let response: EntityPullResponse = serde_json::from_value(json!({
        "results": [{
            "entityType": "topic",
            "ownerType": "agent",
            "ownerId": "agent-a",
            "topicId": "shared",
            "ok": false
        }]
    }))
    .expect("entity result should decode");
    assert_eq!(response.results.len(), 1);
    assert!(serde_json::from_value::<EntityPullResponse>(json!({
        "results": [{"entityType": "topic", "ownerType": "agent", "topicId": "shared", "ok": false}]
    }))
    .is_err());

    let request = MessagePullRequest {
        topics: vec![
            MessagePullTopicSelector {
                owner_type: OwnerType::Agent,
                owner_id: "agent-a".to_string(),
                topic_id: "shared".to_string(),
                message_ids: vec!["m1".to_string()],
            },
            MessagePullTopicSelector {
                owner_type: OwnerType::Group,
                owner_id: "group-a".to_string(),
                topic_id: "shared".to_string(),
                message_ids: vec!["m2".to_string()],
            },
        ],
    };
    request
        .validate()
        .expect("cross-owner same topic id must remain isolated");
}

#[test]
fn phase_stats_and_final_ack_have_strict_wire_shapes() {
    let stats = PhaseStats {
        phase: SyncPhase::Messages,
        expected: 2,
        success: 1,
        errors: 1,
        duration_ms: 25,
    };
    stats.validate().expect("phase stats should validate");
    assert!(PhaseStats {
        success: 2,
        errors: 1,
        ..stats.clone()
    }
    .validate()
    .is_err());
    assert!(serde_json::to_value(PhaseStats {
        duration_ms: 9_007_199_254_740_992,
        ..stats.clone()
    })
    .is_err());

    let ack = FinalAckFrame::new(7, 3, "nonce-1");
    let encoded = serde_json::to_value(&ack).expect("final ack should encode");
    assert_eq!(
        encoded,
        json!({
            "type": "PHASE_ACK",
            "phase": "messages",
            "sessionId": 7,
            "attemptId": 3,
            "nonce": "nonce-1"
        })
    );
    let decoded: FinalAckFrame = serde_json::from_value(encoded).expect("final ack should decode");
    assert_eq!(decoded, ack);
    assert!(serde_json::from_value::<FinalAckFrame>(json!({
        "type": "PHASE_ACK",
        "phase": "messages",
        "sessionId": 7,
        "attemptId": 3,
        "nonce": "nonce-1",
        "extra": true
    }))
    .is_err());
    assert!(serde_json::from_value::<FinalAckFrame>(json!({
        "type": "PHASE_ACK",
        "phase": "owner_metadata",
        "sessionId": 7,
        "attemptId": 3,
        "nonce": "nonce-1"
    }))
    .is_err());
    assert!(serde_json::from_value::<FinalAckFrame>(json!({
        "type": "PHASE_ACK",
        "phase": "messages",
        "sessionId": 9007199254740992u64,
        "attemptId": 3,
        "nonce": "nonce-1"
    }))
    .is_err());
    assert!(serde_json::to_value(FinalAckFrame::new(9_007_199_254_740_992, 3, "nonce-1")).is_err());
}
