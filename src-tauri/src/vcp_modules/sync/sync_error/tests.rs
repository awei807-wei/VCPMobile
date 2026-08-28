use super::codec::{decode_wire_sync_error, encode_http_sync_error_body, WIRE_ERROR_MARKER};
use super::payload::{build_local_error_payload, build_wire_error_payload};
use super::registry::error_definition;
use super::types::{SyncErrorCategory, SyncErrorOrigin, SyncErrorStage, SyncRetryAction};
use super::validation::parse_wire_sync_error;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const FIXTURE_BYTES: &[u8] = include_bytes!("../fixtures/error_contract_1_2_golden.json");
const FIXTURE_SHA256: &str = "434279b33a86a2206c1e4f47caccb4e72f05b2f9d48e093af95d5ebae6947adb";

fn valid_error() -> Value {
    json!({
        "code": "UPSTREAM_EXTENSION_FAILED",
        "origin": "desktop_plugin",
        "stage": "messages",
        "kind": "internal",
        "retry": "manual",
        "message": "diagnostic",
        "failedTopicIds": []
    })
}

#[test]
fn golden_fixture_is_byte_exact_and_covers_valid_invalid_errors() {
    assert_eq!(
        format!("{:x}", Sha256::digest(FIXTURE_BYTES)),
        FIXTURE_SHA256
    );
    let fixture: Value = serde_json::from_slice(FIXTURE_BYTES).expect("golden fixture JSON");
    assert_eq!(fixture["wireProtocol"], "1.2");
    assert_eq!(fixture["pluginVersion"], "1.2.0");

    for entry in fixture["validErrors"].as_array().expect("validErrors") {
        parse_wire_sync_error(&entry["error"]).expect("valid Wire 1.2 error");
    }
    for entry in fixture["invalidErrors"].as_array().expect("invalidErrors") {
        assert!(
            parse_wire_sync_error(&entry["error"]).is_err(),
            "{}",
            entry["name"]
        );
    }
    for (code, semantics) in fixture["registeredSemantics"]
        .as_object()
        .expect("registeredSemantics")
    {
        let semantics = semantics.as_array().expect("semantic tuple");
        let registered = error_definition(code).expect("registered code");
        assert_eq!(
            serde_json::to_value(registered.category).expect("serialize kind"),
            semantics[0],
            "kind for {code}"
        );
        assert_eq!(
            serde_json::to_value(registered.retry).expect("serialize retry"),
            semantics[1],
            "retry for {code}"
        );
    }
}

#[test]
fn wire_error_has_exactly_seven_camel_case_fields_and_denies_unknowns() {
    let parsed = parse_wire_sync_error(&valid_error()).expect("seven-field error");
    assert_eq!(parsed.code, "UPSTREAM_EXTENSION_FAILED");
    assert_eq!(parsed.origin, SyncErrorOrigin::DesktopPlugin);
    assert_eq!(parsed.stage, SyncErrorStage::Messages);
    assert_eq!(parsed.kind, SyncErrorCategory::Internal);
    assert_eq!(parsed.retry, SyncRetryAction::Manual);
    assert_eq!(parsed.message, "diagnostic");
    assert!(parsed.failed_topic_ids.is_empty());

    let mut unknown = valid_error();
    unknown["debug"] = json!("not on the wire");
    assert!(parse_wire_sync_error(&unknown).is_err());

    for field in [
        "code",
        "origin",
        "stage",
        "kind",
        "retry",
        "message",
        "failedTopicIds",
    ] {
        let mut missing = valid_error();
        missing.as_object_mut().expect("object").remove(field);
        assert!(parse_wire_sync_error(&missing).is_err(), "missing {field}");
    }
}

#[test]
fn enum_values_are_closed_sets() {
    for (field, value) in [
        ("origin", "backend"),
        ("stage", "sync"),
        ("kind", "fatal"),
        ("retry", "retry_later"),
    ] {
        let mut invalid = valid_error();
        invalid[field] = json!(value);
        assert!(parse_wire_sync_error(&invalid).is_err(), "invalid {field}");
    }
}

#[test]
fn stable_code_rules_reject_platform_and_local_error_codes() {
    for code in [
        "",
        "desktop raw code",
        "lowercase_code",
        "ERR_NETWORK",
        "SQLITE_BUSY",
        "EAI_AGAIN",
        "ENOENT",
        "EACCES",
        "ETIMEDOUT",
    ] {
        let mut invalid = valid_error();
        invalid["code"] = json!(code);
        assert!(
            parse_wire_sync_error(&invalid).is_err(),
            "invalid code {code:?}"
        );
    }

    for code in ["A", "UPSTREAM_EXTENSION_FAILED", "CODE_123"] {
        let mut valid = valid_error();
        valid["code"] = json!(code);
        assert!(parse_wire_sync_error(&valid).is_ok(), "stable code {code}");
    }
}

#[test]
fn known_codes_lock_kind_and_retry_while_unknown_codes_keep_metadata() {
    let mut known = valid_error();
    known["code"] = json!("POWER_SAVE_MODE");
    known["origin"] = json!("mobile_native");
    known["stage"] = json!("preflight");
    known["kind"] = json!("compatibility");
    known["retry"] = json!("after_user_action");
    assert!(parse_wire_sync_error(&known)
        .expect_err("known category conflict")
        .contains("conflicts with its registered code"));

    let mut unknown = valid_error();
    unknown["code"] = json!("UPSTREAM_EXTENSION_FAILED");
    unknown["origin"] = json!("desktop_cds");
    unknown["stage"] = json!("finalize");
    unknown["kind"] = json!("storage");
    unknown["retry"] = json!("automatic");
    let parsed = parse_wire_sync_error(&unknown).expect("unknown stable code");
    assert_eq!(parsed.origin, SyncErrorOrigin::DesktopCds);
    assert_eq!(parsed.stage, SyncErrorStage::Finalize);
    assert_eq!(parsed.kind, SyncErrorCategory::Storage);
    assert_eq!(parsed.retry, SyncRetryAction::Automatic);
}

#[test]
fn message_and_failed_topic_id_boundaries_use_unicode_scalar_counts() {
    let mut valid = valid_error();
    valid["message"] = json!("🙂".repeat(1024));
    valid["failedTopicIds"] = json!(["🙂".repeat(512)]);
    assert!(parse_wire_sync_error(&valid).is_ok());

    valid["message"] = json!("🙂".repeat(1025));
    assert!(parse_wire_sync_error(&valid).is_err());

    valid["message"] = json!("ok");
    valid["failedTopicIds"] = json!(["🙂".repeat(513)]);
    assert!(parse_wire_sync_error(&valid).is_err());

    valid["failedTopicIds"] = json!(vec!["topic"; 9]);
    assert!(parse_wire_sync_error(&valid).is_err());
    valid["failedTopicIds"] = json!(["topic-a", "topic-a"]);
    assert!(parse_wire_sync_error(&valid).is_err());
}

#[test]
fn local_and_remote_payloads_preserve_safe_metadata_and_bound_ids() {
    let local = build_local_error_payload(
        "ENOENT",
        vec!["topic-a".to_owned(), "topic-a".to_owned(), "".to_owned()],
        None,
    );
    assert_eq!(local.code, "SYNC_ATTEMPT_FAILED");
    assert_eq!(local.failed_topic_ids, vec!["topic-a"]);

    let wire = parse_wire_sync_error(&json!({
        "code": "UPSTREAM_EXTENSION_FAILED",
        "origin": "desktop_cds",
        "stage": "finalize",
        "kind": "storage",
        "retry": "automatic",
        "message": "remote diagnostic",
        "failedTopicIds": ["topic-a"]
    }))
    .expect("unknown stable wire error");
    let payload = build_wire_error_payload(&wire, vec!["topic-b".to_owned()], None);
    assert_eq!(payload.code, "UPSTREAM_EXTENSION_FAILED");
    assert_eq!(payload.origin, SyncErrorOrigin::DesktopCds);
    assert_eq!(payload.stage, SyncErrorStage::Finalize);
    assert_eq!(payload.category, SyncErrorCategory::Storage);
    assert_eq!(payload.retry_action, SyncRetryAction::Automatic);
    assert_eq!(payload.failed_topic_ids, vec!["topic-a", "topic-b"]);
}

#[test]
fn error_codecs_reject_duplicate_keys_and_trailing_json() {
    let valid = serde_json::to_string(&json!({ "error": valid_error() })).expect("valid body");
    assert!(encode_http_sync_error_body(valid.as_bytes())
        .expect("strict HTTP body")
        .is_some());

    for body in [
        r#"{"error":{"code":"A","code":"B"}}"#,
        r#"{"error":null} {"trailing":true}"#,
    ] {
        assert!(encode_http_sync_error_body(body.as_bytes()).is_err());
    }

    let valid_marker = format!("{WIRE_ERROR_MARKER}{}", valid_error());
    assert!(decode_wire_sync_error(&valid_marker).is_some());
    for encoded in [
        format!(
            "{WIRE_ERROR_MARKER}{}",
            r#"{"code":"A","code":"B","origin":"desktop_plugin","stage":"startup","kind":"internal","retry":"manual","message":"x","failedTopicIds":[]}"#
        ),
        format!("{valid_marker} {{\"trailing\":true}}"),
    ] {
        assert!(decode_wire_sync_error(&encoded).is_none());
    }
}
