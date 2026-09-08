use super::{parse_helper_endpoint, serialize_helper_command};
use serde_json::json;

#[test]
fn endpoint_requires_version_port_and_token() {
    let endpoint = parse_helper_endpoint(r#"{"version":1,"port":38417,"token":"abc"}"#)
        .expect("valid helper endpoint");
    assert_eq!(endpoint.port, 38417);
    assert_eq!(endpoint.token, "abc");
    assert!(parse_helper_endpoint("38417").is_err());
    assert!(parse_helper_endpoint(r#"{"version":2,"port":38417,"token":"abc"}"#).is_err());
    assert!(parse_helper_endpoint(r#"{"version":1,"port":38417}"#).is_err());
}

#[test]
fn command_serialization_injects_token_and_restores_identity() {
    let command = serialize_helper_command(
        "query",
        "message-1",
        "agent",
        "owner-1",
        "topic-1",
        Some(&json!({
            "token": "attacker-token",
            "messageId": "other-message",
            "startIndex": 3
        })),
        "instance-token",
    )
    .expect("serialize helper command");
    let value: serde_json::Value = serde_json::from_str(&command).unwrap();
    assert_eq!(value["token"], "instance-token");
    assert_eq!(value["messageId"], "message-1");
    assert_eq!(value["requestId"], "message-1");
    assert_eq!(value["startIndex"], 3);
}

#[test]
fn every_helper_action_injects_current_token_and_composite_identity() {
    for action in [
        "start",
        "query",
        "prepare_resume",
        "resume",
        "cancel_resume",
        "stop",
    ] {
        let command = serialize_helper_command(
            action,
            "message-current",
            "group",
            "owner-current",
            "topic-current",
            Some(&json!({
                "token": "attacker-token",
                "requestId": "attacker-request",
                "messageId": "attacker-message",
                "ownerType": "agent",
                "ownerId": "attacker-owner",
                "topicId": "attacker-topic",
                "expectedGeneration": 9,
                "requestEpoch": 17
            })),
            "instance-token",
        )
        .expect("serialize helper command");
        let value: serde_json::Value = serde_json::from_str(&command).unwrap();
        assert_eq!(value["token"], "instance-token", "{action}");
        assert_eq!(value["requestId"], "message-current", "{action}");
        assert_eq!(value["messageId"], "message-current", "{action}");
        assert_eq!(value["ownerType"], "group", "{action}");
        assert_eq!(value["ownerId"], "owner-current", "{action}");
        assert_eq!(value["topicId"], "topic-current", "{action}");
        assert_eq!(value["expectedGeneration"], 9, "{action}");
        assert_eq!(value["requestEpoch"], 17, "{action}");
    }
}

#[test]
fn cancel_resume_serialization_keeps_complete_identity_and_generation() {
    let command = serialize_helper_command(
        "cancel_resume",
        "message-cancel",
        "agent",
        "owner-cancel",
        "topic-cancel",
        Some(&json!({"expectedGeneration": 27})),
        "instance-token",
    )
    .expect("serialize cancel_resume command");
    let value: serde_json::Value = serde_json::from_str(&command).unwrap();
    assert_eq!(value["requestId"], "message-cancel");
    assert_eq!(value["messageId"], "message-cancel");
    assert_eq!(value["ownerType"], "agent");
    assert_eq!(value["ownerId"], "owner-cancel");
    assert_eq!(value["topicId"], "topic-cancel");
    assert_eq!(value["expectedGeneration"], 27);
    assert_eq!(value["token"], "instance-token");
}
