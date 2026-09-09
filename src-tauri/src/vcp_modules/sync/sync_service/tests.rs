use super::batching::{build_diff_batches, MAX_MESSAGES_PER_BATCH, MAX_WS_DIFF_BATCH_BYTES};
use super::connection_config::resolve_connection_settings;
use super::diagnostics::{check_loopback_on_mobile, diagnose_connection_failure};
use super::errors::{build_sync_error_payload, encode_sync_command_error};
use super::frames::{is_valid_intermediate_ack, validate_changed_topics};
use super::lifecycle::cancel_and_join_session;
use super::lifecycle::create_session_command_channel;
use super::logs::count_log_removal;
use super::protocol::{
    consume_final_ack, enforce_final_ack_deadline, enforce_manifest_response_deadline,
    enforce_topic_hash_response_deadline, parse_unique_nonempty_strings,
    parse_version_handshake_payload, protocol_send_failure_message, take_retry_slot, RetryBudget,
    VersionHandshakeError, MAX_SYNC_RETRIES, MAX_SYNC_TOPICS,
};
use super::session_support::perform_handshake;
use super::types::{FinalAckKey, MessagePhaseBarrier, SyncSessionHandle, SyncTaskTracker};
use super::*;
use crate::vcp_modules::settings_manager::{ConnectionProfile, Settings};
use crate::vcp_modules::sync_error::decode_wire_sync_error;
use crate::vcp_modules::sync_error::SyncErrorCategory;
use crate::vcp_modules::sync_types::{
    DeleteNotificationFrame, DeleteTarget, ManifestType, MessageLiveState, MessageVersionState,
    TopicDiffResultFrame,
};
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::error::Error as WsError;
use tokio_tungstenite::tungstenite::http::{Response, StatusCode};
use tokio_util::sync::CancellationToken;

#[test]
fn connection_settings_use_the_selected_profile_and_freeze_its_values() {
    let settings = Settings {
        active_connection_profile_id: "wan".to_string(),
        sync_server_url: "ws://stale.example/ws".to_string(),
        sync_http_url: "http://stale.example".to_string(),
        sync_token: "stale".to_string(),
        connection_profiles: vec![ConnectionProfile {
            id: "wan".to_string(),
            name: "外网".to_string(),
            sync_server_url: "wss://sync.example/ws?device=mobile&token=old".to_string(),
            sync_http_url: "https://sync.example/".to_string(),
            sync_token: "a token&value".to_string(),
            ..ConnectionProfile::default()
        }],
        ..Settings::default()
    };

    let resolved = resolve_connection_settings(&settings, false).expect("resolve selected profile");

    assert_eq!(resolved.profile_id, "wan");
    assert_eq!(resolved.http_url, "https://sync.example");
    assert_eq!(resolved.token, "a token&value");
    let url = url::Url::parse(&resolved.ws_url).expect("parse resolved websocket URL");
    let query = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        query.get("device").map(|value| value.as_ref()),
        Some("mobile")
    );
    assert_eq!(
        query.get("token").map(|value| value.as_ref()),
        Some("a token&value")
    );
}

#[test]
fn connection_settings_reject_a_missing_selected_profile() {
    let settings = Settings {
        active_connection_profile_id: "wan".to_string(),
        connection_profiles: vec![ConnectionProfile {
            id: "lan".to_string(),
            ..ConnectionProfile::default()
        }],
        ..Settings::default()
    };

    let error = match resolve_connection_settings(&settings, false) {
        Ok(_) => panic!("missing profile must fail"),
        Err(error) => error,
    };
    assert!(error.contains("wan"));
}

#[test]
fn connection_settings_keep_legacy_global_fallback_without_profiles() {
    let settings = Settings {
        active_connection_profile_id: "lan".to_string(),
        sync_server_url: "ws://192.0.2.10:5975/ws".to_string(),
        sync_http_url: "http://192.0.2.10:5975".to_string(),
        sync_token: "legacy-token".to_string(),
        ..Settings::default()
    };

    let resolved = resolve_connection_settings(&settings, false).expect("resolve legacy settings");

    assert_eq!(resolved.profile_id, "lan");
    assert_eq!(resolved.http_url, "http://192.0.2.10:5975");
}

#[test]
fn connection_settings_reject_duplicate_profiles_and_unsafe_urls() {
    let profile = ConnectionProfile {
        id: "lan".to_string(),
        sync_server_url: "ws://sync.example:5975".to_string(),
        sync_http_url: "http://sync.example:5974".to_string(),
        sync_token: "token".to_string(),
        ..ConnectionProfile::default()
    };
    let duplicate = Settings {
        active_connection_profile_id: "lan".to_string(),
        connection_profiles: vec![profile.clone(), profile.clone()],
        ..Settings::default()
    };
    assert!(resolve_connection_settings(&duplicate, false).is_err());

    for (ws_url, http_url) in [
        ("ws://user:pass@sync.example", "http://sync.example"),
        ("ws://sync.example#fragment", "http://sync.example"),
        ("ws://sync.example", "http://sync.example?token=unsafe"),
    ] {
        let settings = Settings {
            active_connection_profile_id: "lan".to_string(),
            connection_profiles: vec![ConnectionProfile {
                sync_server_url: ws_url.to_string(),
                sync_http_url: http_url.to_string(),
                ..profile.clone()
            }],
            ..Settings::default()
        };
        assert!(resolve_connection_settings(&settings, false).is_err());
    }
}

#[test]
fn connection_settings_reject_android_loopback_endpoints() {
    let settings = Settings {
        active_connection_profile_id: "lan".to_string(),
        connection_profiles: vec![ConnectionProfile {
            id: "lan".to_string(),
            sync_server_url: "ws://127.0.0.1:5975".to_string(),
            sync_http_url: "http://10.0.2.2:5974".to_string(),
            sync_token: "token".to_string(),
            ..ConnectionProfile::default()
        }],
        ..Settings::default()
    };

    assert!(resolve_connection_settings(&settings, false).is_ok());
    assert!(resolve_connection_settings(&settings, true).is_err());
}

#[test]
fn sync_error_contract_keeps_raw_detail_out_of_the_user_payload() {
    let payload = build_sync_error_payload(
        "TOKEN_MISMATCH",
        vec!["topic-a".to_string()],
        Some("20260813_120000_000_7_sync.log".to_string()),
    );
    let json = serde_json::to_value(payload).expect("serialize sync error");

    assert_eq!(json["category"], "configuration");
    assert_eq!(json["origin"], "mobile_sync");
    assert_eq!(json["stage"], "connect");
    assert_eq!(json["retryAction"], "after_user_action");
    assert_eq!(json["message"], "手机端与电脑端的同步令牌不一致");
    assert_eq!(json["guidance"], "重新核对两端令牌后再试。");
    assert!(json.get("detail").is_none());
    assert!(json.get("solution").is_none());

    let fallback = build_sync_error_payload("desktop raw code", Vec::new(), None);
    assert_eq!(fallback.code, "SYNC_ATTEMPT_FAILED");
    assert_eq!(fallback.category, SyncErrorCategory::Internal);
}

#[test]
fn sync_error_classification_covers_connection_protocol_and_data_failures() {
    assert_eq!(
        build_sync_error_payload("NETWORK_TIMEOUT", Vec::new(), None).category,
        SyncErrorCategory::Connection
    );
    assert_eq!(
        build_sync_error_payload("PROTOCOL_FRAME_INVALID", Vec::new(), None).category,
        SyncErrorCategory::Protocol
    );
    assert_eq!(
        build_sync_error_payload("SYNC_DB_DRAIN_FAILED", Vec::new(), None).category,
        SyncErrorCategory::Storage
    );
    assert_eq!(
        build_sync_error_payload("SYNC_VERSION_INCOMPATIBLE", Vec::new(), None).category,
        SyncErrorCategory::Compatibility
    );
}

#[test]
fn command_errors_use_the_structured_transport_prefix() {
    let encoded = encode_sync_command_error(
        "SYNC_ACTIVE_GENERATION",
        "Bearer raw-secret should stay in native logs only",
    );
    let json = encoded
        .strip_prefix("SYNC_ERROR:")
        .expect("structured sync command prefix");
    let payload: Value = serde_json::from_str(json).expect("structured sync error JSON");

    assert_eq!(payload["code"], "SYNC_ACTIVE_GENERATION");
    assert_eq!(payload["category"], "connection");
    assert_eq!(payload["retryAction"], "manual");
    assert!(!encoded.contains("raw-secret"));
}

#[test]
fn log_cleanup_never_counts_failed_removals_as_removed() {
    let mut removed = 0;
    let mut failed = 0;
    assert!(count_log_removal(Ok(()), &mut removed, &mut failed).is_none());
    assert!(count_log_removal(
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied"
        )),
        &mut removed,
        &mut failed,
    )
    .is_some());
    assert_eq!(removed, 1);
    assert_eq!(failed, 1);
}

#[test]
fn protocol_send_failure_names_frame_and_transport_error() {
    assert_eq!(
        protocol_send_failure_message("owner metadata manifest", "socket closed"),
        "Failed to send owner metadata manifest: socket closed"
    );
}

#[test]
fn retry_budget_is_shared_across_connection_stages() {
    let mut retry_budget = RetryBudget::new();
    assert_eq!(
        take_retry_slot(&mut retry_budget),
        Some(Duration::from_millis(500))
    );
    assert_eq!(
        take_retry_slot(&mut retry_budget),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        take_retry_slot(&mut retry_budget),
        Some(Duration::from_secs(2))
    );
    assert_eq!(take_retry_slot(&mut retry_budget), None);
    assert_eq!(retry_budget.attempts(), MAX_SYNC_RETRIES);
}

#[tokio::test]
async fn missing_manifest_frame_fails_the_current_attempt() {
    let expected = Arc::new(Mutex::new(HashSet::from([
        ManifestType::Owner,
        ManifestType::Avatar,
    ])));
    let phase = Arc::new(AtomicU8::new(1));
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
    enforce_manifest_response_deadline(expected, phase, 1, tx, 7, Duration::from_millis(1)).await;
    match rx.recv().await.expect("deadline command") {
        SyncCommand::FailAttemptDetailed {
            attempt_id,
            code,
            message,
            ..
        } => {
            assert_eq!(attempt_id, 7);
            assert_eq!(code, "MANIFEST_RESPONSE_TIMEOUT");
            assert!(message.contains("owner"));
            assert!(message.contains("avatar"));
        }
        _ => panic!("unexpected deadline command"),
    }
}

#[tokio::test]
async fn missing_topic_hash_frame_fails_the_current_attempt() {
    let expected = Arc::new(AsyncMutex::new(Some(HashSet::from([TopicKey::new(
        "agent", "agent-a", "topic-a",
    )]))));
    let phase = Arc::new(AtomicU8::new(3));
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();
    enforce_topic_hash_response_deadline(expected, phase, 3, tx, 8, Duration::from_millis(1)).await;
    match rx.recv().await.expect("deadline command") {
        SyncCommand::FailAttemptDetailed {
            attempt_id, code, ..
        } => {
            assert_eq!(attempt_id, 8);
            assert_eq!(code, "TOPIC_HASH_RESPONSE_TIMEOUT");
        }
        _ => panic!("unexpected deadline command"),
    }
}

#[test]
fn protocol_1_4_version_ack_is_strict_and_uses_public_field_names() {
    let ack = parse_version_handshake_payload(
        r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":"1.4"}"#,
    )
    .expect("strict 1.4 acknowledgement")
    .expect("version acknowledgement frame");
    assert_eq!(ack.plugin_version, "1.4.0");
    assert_eq!(ack.protocol_version, "1.4");

    assert!(
        parse_version_handshake_payload(r#"{"type":"VERSION_ACK","version":"1.4.0"}"#).is_err()
    );
    assert!(parse_version_handshake_payload(
        r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":1.4}"#
    )
    .is_err());
    assert!(parse_version_handshake_payload(
            r#"{"type":"VERSION_ACK","type":"SYNC_LOG_EVENT","pluginVersion":"1.4.0","protocolVersion":"1.4"}"#
        )
        .is_err());
}

#[test]
fn handshake_preserves_a_structured_desktop_error_before_version_ack() {
    let result = parse_version_handshake_payload(
        r#"{"type":"SYNC_ERROR","error":{"code":"PLUGIN_VERSION_MISMATCH","origin":"desktop_plugin","stage":"handshake","kind":"compatibility","retry":"after_user_action","message":"plugin package mismatch","failedTopicIds":[]}}"#,
    );
    let VersionHandshakeError::Remote(encoded) = result.expect_err("remote error") else {
        panic!("expected structured remote error");
    };
    assert_eq!(
        decode_wire_sync_error(&encoded)
            .expect("encoded error")
            .code,
        "PLUGIN_VERSION_MISMATCH"
    );
    assert!(parse_version_handshake_payload(
        r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":1}"#
    )
    .expect("strict pre-ack desktop log")
    .is_none());
    assert!(
        parse_version_handshake_payload(r#"{"type":"SYNC_LOG_EVENT","level":"info"}"#).is_err()
    );
}

#[tokio::test]
async fn handshake_accepts_the_current_linux_pre_ack_log_sequence() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local websocket listener");
    let address = listener.local_addr().expect("read listener address");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept websocket client");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("accept websocket handshake");
        ws.send(Message::Text(
            r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":1}"#
                .into(),
        ))
        .await
        .expect("send pre-ack diagnostic log");

        let Message::Text(version_check) = ws
            .next()
            .await
            .expect("receive version check")
            .expect("read version check")
        else {
            panic!("version check must be text");
        };
        let payload = crate::vcp_modules::wire_protocol::parse_strict_json(&version_check)
            .expect("parse strict version check");
        assert_eq!(
            payload,
            json!({
                "type": "VERSION_CHECK",
                "mobileVersion": env!("CARGO_PKG_VERSION"),
                "protocolVersion": "1.4"
            })
        );

        ws.send(Message::Text(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":"1.4"}"#.into(),
        ))
        .await
        .expect("send version acknowledgement");
        ws.close(None).await.expect("close websocket");
    });

    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
        .await
        .expect("connect local websocket");
    let mut established = perform_handshake(client, &CancellationToken::new())
        .await
        .expect("complete strict handshake after diagnostic log");
    established
        .close(None)
        .await
        .expect("close client websocket");
    server.await.expect("join websocket server");
}

#[test]
fn changed_topic_list_rejects_wrong_types_empty_ids_and_duplicates() {
    assert_eq!(
        parse_unique_nonempty_strings(
            &json!(["topic-a", "topic-b"]),
            "changedTopics",
            MAX_SYNC_TOPICS,
        )
        .expect("valid topic list"),
        vec!["topic-a".to_string(), "topic-b".to_string()]
    );
    assert!(
        parse_unique_nonempty_strings(&json!("topic-a"), "changedTopics", MAX_SYNC_TOPICS,)
            .is_err()
    );
    assert!(
        parse_unique_nonempty_strings(&json!([""]), "changedTopics", MAX_SYNC_TOPICS,).is_err()
    );
    assert!(parse_unique_nonempty_strings(
        &json!(["topic-a", "topic-a"]),
        "changedTopics",
        MAX_SYNC_TOPICS,
    )
    .is_err());
    assert!(
        parse_unique_nonempty_strings(&json!(["topic-a", "topic-b"]), "changedTopics", 1,).is_err()
    );
}

#[test]
fn topic_diff_results_and_intermediate_acks_require_exact_current_shapes() {
    let topic_a = TopicKey::new("agent", "agent-a", "topic-a");
    let topic_b = TopicKey::new("group", "group-b", "topic-b");
    let expected = HashSet::from([topic_a.clone(), topic_b.clone()]);
    let result: TopicDiffResultFrame = serde_json::from_value(json!({
        "type":"SYNC_TOPIC_DIFF_RESULT",
        "changedTopics":[{
            "ownerType":"group",
            "ownerId":"group-b",
            "topicId":"topic-b"
        }]
    }))
    .expect("exact topic diff result");
    assert_eq!(
        validate_changed_topics(result.changed_topics, &expected)
            .expect("expected changed identity"),
        vec![topic_b]
    );
    assert!(validate_changed_topics(
        vec![TopicKey::new("agent", "agent-a", "topic-c")],
        &expected,
    )
    .is_err());
    assert!(serde_json::from_value::<TopicDiffResultFrame>(json!({
        "type":"SYNC_TOPIC_DIFF_RESULT",
        "changedTopics":[],
        "debug":true
    }))
    .is_err());

    assert!(is_valid_intermediate_ack(
        &json!({"type":"PHASE_ACK","phase":"owner_metadata"})
    ));
    for payload in [
        json!({"type":"PHASE_ACK","phase":"topic_validation"}),
        json!({"type":"PHASE_ACK","phase":"messages","debug":true}),
        json!({"type":"PHASE_ACK","phase":3}),
    ] {
        assert!(!is_valid_intermediate_ack(&payload));
    }
}

#[test]
fn phase3_diff_batches_enforce_serialized_byte_budget() {
    use crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState;
    use std::collections::{BTreeMap, HashMap};

    let mut states = HashMap::new();
    for index in 0..3 {
        states.insert(
            TopicKey::new("agent", "agent-a", format!("topic-{index}")),
            TopicLocalState {
                content_hash: "a".repeat(64),
                messages: BTreeMap::from([(
                    format!("message-{index}-{}", "x".repeat(3 * 1024 * 1024)),
                    MessageVersionState::Live(MessageLiveState {
                        message_hash: "b".repeat(64),
                        updated_at: 7,
                    }),
                )]),
            },
        );
    }
    let batches = build_diff_batches(states).expect("bounded batches");
    assert!(batches.len() >= 2);
    for batch in batches {
        let bytes = serde_json::to_vec(&json!({
            "type": "SYNC_MESSAGE_DIFF_REQUEST",
            "topics": batch.topics,
        }))
        .expect("serialize batch");
        assert!(bytes.len() <= MAX_WS_DIFF_BATCH_BYTES);
    }

    let oversized = HashMap::from([(
        TopicKey::new("agent", "agent-a", "topic-oversized"),
        TopicLocalState {
            content_hash: "a".repeat(64),
            messages: BTreeMap::from([(
                "x".repeat(MAX_WS_DIFF_BATCH_BYTES),
                MessageVersionState::Live(MessageLiveState {
                    message_hash: "b".repeat(64),
                    updated_at: 7,
                }),
            )]),
        },
    )]);
    assert!(build_diff_batches(oversized).is_err());

    let too_many_messages = (0..=MAX_MESSAGES_PER_BATCH)
        .map(|index| {
            (
                format!("message-{index}"),
                MessageVersionState::Live(MessageLiveState {
                    message_hash: "b".repeat(64),
                    updated_at: 7,
                }),
            )
        })
        .collect();
    let oversized_topic = HashMap::from([(
        TopicKey::new("agent", "agent-a", "topic-too-many"),
        TopicLocalState {
            content_hash: "a".repeat(64),
            messages: too_many_messages,
        },
    )]);
    assert!(build_diff_batches(oversized_topic).is_err());
}

#[tokio::test]
async fn session_task_tracker_cancels_and_joins_children() {
    let cancel_token = CancellationToken::new();
    let tracker = Arc::new(SyncTaskTracker::new(cancel_token.clone()));
    let started = Arc::new(tokio::sync::Notify::new());
    let late_side_effect = Arc::new(AtomicBool::new(false));
    let child_started = started.clone();
    let child_side_effect = late_side_effect.clone();

    tracker
        .spawn(async move {
            child_started.notify_one();
            tokio::time::sleep(Duration::from_secs(30)).await;
            child_side_effect.store(true, Ordering::SeqCst);
        })
        .await;
    started.notified().await;

    cancel_token.cancel();
    tracker.close_and_wait().await;

    assert!(!late_side_effect.load(Ordering::SeqCst));
}

#[tokio::test]
async fn cancel_session_waits_for_main_task_exit() {
    let cancel_token = CancellationToken::new();
    let task_token = cancel_token.clone();
    let exited = Arc::new(AtomicBool::new(false));
    let task_exited = exited.clone();
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let join_handle = tokio::spawn(async move {
        task_token.cancelled().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        task_exited.store(true, Ordering::SeqCst);
        Ok(())
    });

    cancel_and_join_session(SyncSessionHandle {
        session_id: 1,
        cancel_token,
        command_tx,
        join_handle,
    })
    .await
    .expect("session should exit cleanly");

    assert!(exited.load(Ordering::SeqCst));
}

#[tokio::test]
async fn final_ack_deadline_fails_only_the_current_pending_attempt() {
    let expected = FinalAckKey {
        session_id: 3,
        attempt_id: 7,
        phase: "messages".into(),
        nonce: "nonce-7".into(),
    };
    let pending = Arc::new(Mutex::new(Some(expected.clone())));
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();

    enforce_final_ack_deadline(pending.clone(), expected, tx, Duration::from_millis(1)).await;

    match rx.try_recv() {
        Ok(SyncCommand::FailAttempt {
            attempt_id,
            code,
            message,
        }) => {
            assert_eq!(attempt_id, 7);
            assert_eq!(code, "FINAL_ACK_TIMEOUT");
            assert!(message.contains("acknowledgement timed out"));
        }
        _ => panic!("pending final acknowledgement must fail the current attempt"),
    }
    assert!(pending.lock().expect("pending lock").is_none());
}

#[tokio::test]
async fn stale_watchdog_cannot_clear_a_newer_attempt() {
    let stale = FinalAckKey {
        session_id: 3,
        attempt_id: 7,
        phase: "messages".into(),
        nonce: "nonce-7".into(),
    };
    let current = FinalAckKey {
        session_id: 3,
        attempt_id: 8,
        phase: "messages".into(),
        nonce: "nonce-8".into(),
    };
    let pending = Arc::new(Mutex::new(Some(current.clone())));
    let (tx, mut rx) = mpsc::unbounded_channel::<SyncCommand>();

    enforce_final_ack_deadline(pending.clone(), stale, tx, Duration::from_millis(1)).await;

    assert!(rx.try_recv().is_err());
    assert_eq!(
        pending.lock().expect("pending lock").as_ref(),
        Some(&current)
    );
}

#[test]
fn final_ack_requires_exact_identity_and_is_consumed_once() {
    let expected = FinalAckKey {
        session_id: 11,
        attempt_id: 4,
        phase: "messages".into(),
        nonce: "exact-nonce".into(),
    };
    let pending = Arc::new(Mutex::new(Some(expected.clone())));
    let mismatches = [
        json!({"type":"PHASE_COMPLETED","phase":"messages","sessionId":11,"attemptId":4,"nonce":"exact-nonce"}),
        json!({"type":"PHASE_ACK","phase":"owner_metadata","sessionId":11,"attemptId":4,"nonce":"exact-nonce"}),
        json!({"type":"PHASE_ACK","phase":"messages","sessionId":10,"attemptId":4,"nonce":"exact-nonce"}),
        json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":3,"nonce":"exact-nonce"}),
        json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":4,"nonce":"stale-nonce"}),
        json!({"type":"PHASE_ACK","phase":"messages","sessionId":11,"attemptId":4}),
    ];
    for payload in mismatches {
        assert!(!consume_final_ack(&pending, &payload));
        assert_eq!(
            pending.lock().expect("pending lock").as_ref(),
            Some(&expected)
        );
    }

    let exact = json!({
        "type": "PHASE_ACK",
        "phase": "messages",
        "sessionId": 11,
        "attemptId": 4,
        "nonce": "exact-nonce"
    });
    assert!(consume_final_ack(&pending, &exact));
    assert!(!consume_final_ack(&pending, &exact));
    assert!(pending.lock().expect("pending lock").is_none());
}

#[test]
fn final_ack_rejects_unknown_fields_even_when_identity_matches() {
    let expected = FinalAckKey {
        session_id: 11,
        attempt_id: 4,
        phase: "messages".into(),
        nonce: "exact-nonce".into(),
    };
    assert!(!expected.matches_payload(&json!({
        "type": "PHASE_ACK",
        "phase": "messages",
        "sessionId": 11,
        "attemptId": 4,
        "nonce": "exact-nonce",
        "debug": true
    })));
}

#[test]
fn finalization_waits_for_the_messages_phase_ack_in_both_event_orders() {
    let mut command_first = MessagePhaseBarrier::default();
    assert!(command_first.defer_finalize());
    assert!(command_first.acknowledge());
    assert!(!command_first.defer_finalize());

    let mut ack_first = MessagePhaseBarrier::default();
    assert!(!ack_first.acknowledge());
    assert!(!ack_first.defer_finalize());

    ack_first.reset();
    assert!(ack_first.defer_finalize());
}

#[test]
fn command_router_tracks_the_current_session_owner() {
    let router = SyncCommandRouter::default();
    assert!(router.send(SyncCommand::Cancel).is_err());

    let (first_tx, _first_rx) = mpsc::unbounded_channel::<SyncCommand>();
    router.install(1, first_tx);
    let (second_tx, mut second_rx) = mpsc::unbounded_channel::<SyncCommand>();
    router.install(2, second_tx);

    router.clear_if_owner(1);
    router
        .send(SyncCommand::Cancel)
        .expect("stale cleanup must preserve the current session sender");
    assert!(matches!(second_rx.try_recv(), Ok(SyncCommand::Cancel)));

    router.clear_if_owner(2);
    assert!(router.send(SyncCommand::Cancel).is_err());
}

#[test]
fn a_new_session_queues_the_initial_owner_manifest_command() {
    let (_tx, mut rx) = create_session_command_channel().expect("create session channel");
    assert!(matches!(rx.try_recv(), Ok(SyncCommand::StartManualSync)));
    assert!(rx.try_recv().is_err());
}

#[test]
fn delete_notifications_require_complete_wire_1_4_identity() {
    let frame = DeleteNotificationFrame::new(
        DeleteTarget::Topic(TopicKey::new("agent", "agent-a", "topic-a")),
        7,
    );
    assert_eq!(
        serde_json::to_value(frame).expect("serialize delete notification"),
        json!({
            "type": "SYNC_ENTITY_DELETE",
            "targetType": "topic",
            "ownerType": "agent",
            "ownerId": "agent-a",
            "topicId": "topic-a",
            "deletedAt": 7,
        })
    );
}

#[test]
fn test_check_loopback_on_mobile() {
    // Test Android mode (is_android = true)
    assert!(check_loopback_on_mobile("ws://127.0.0.1:3000", true));
    assert!(check_loopback_on_mobile(
        "ws://localhost:8080/ws-sync",
        true
    ));
    assert!(!check_loopback_on_mobile("ws://192.168.1.100:3000", true));
    assert!(!check_loopback_on_mobile("ws://my-pc.local:3000", true));

    // Test non-Android mode (is_android = false)
    assert!(!check_loopback_on_mobile("ws://127.0.0.1:3000", false));
    assert!(!check_loopback_on_mobile(
        "ws://localhost:8080/ws-sync",
        false
    ));
}

#[tokio::test]
async fn test_diagnose_unauthorized_token() {
    // Create an HTTP Error with status 401
    let response = Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .body(None)
        .unwrap();
    let err = WsError::Http(response);

    let diagnosis = diagnose_connection_failure(
        "ws://192.168.1.100:3000/ws-sync",
        "http://192.168.1.100:3000",
        &err,
    )
    .await;

    assert_eq!(diagnosis.error_code, "TOKEN_MISMATCH");
    assert!(diagnosis.error_message.contains("身份认证失败"));
    assert!(diagnosis.solution.contains("同步令牌"));
}

#[tokio::test]
async fn test_diagnose_not_found_path() {
    // Create an HTTP Error with status 404
    let response = Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(None)
        .unwrap();
    let err = WsError::Http(response);

    let diagnosis = diagnose_connection_failure(
        "ws://192.168.1.100:3000/ws-sync",
        "http://192.168.1.100:3000",
        &err,
    )
    .await;

    assert_eq!(diagnosis.error_code, "WS_PATH_INVALID");
    assert!(diagnosis.error_message.contains("路径不存在"));
}

#[tokio::test]
async fn test_diagnose_connection_refused() {
    // Simulate a connection refused error on localhost on port 1.
    let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused");
    let err = WsError::Io(io_err);

    let diagnosis =
        diagnose_connection_failure("ws://127.0.0.1:1/ws-sync", "http://127.0.0.1:1", &err).await;

    assert!(
        diagnosis.error_code == "CONNECTION_REFUSED" || diagnosis.error_code == "NETWORK_TIMEOUT"
    );
    assert!(
        diagnosis.error_message.contains("连接被拒绝")
            || diagnosis.error_message.contains("连接超时")
    );
    assert!(diagnosis.solution.contains("启动") || diagnosis.solution.contains("同一个 WiFi"));
}

#[tokio::test]
async fn test_diagnose_network_unreachable() {
    // Simulate an unreachable address error.
    let io_err = std::io::Error::new(
        std::io::ErrorKind::AddrNotAvailable,
        "address not available",
    );
    let err = WsError::Io(io_err);

    let diagnosis = diagnose_connection_failure(
        "ws://non-existent-domain-vcp-test.xyz/ws-sync",
        "http://non-existent-domain-vcp-test.xyz",
        &err,
    )
    .await;

    assert_eq!(diagnosis.error_code, "NETWORK_UNREACHABLE");
    assert!(
        diagnosis.error_message.contains("网络不可达")
            || diagnosis.error_message.contains("地址无效")
    );
    assert!(diagnosis.solution.contains("网络状态"));
}
