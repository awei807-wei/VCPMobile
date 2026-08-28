//! Strict dispatch for the two frames allowed during the Wire 1.2 handshake.

use super::{parse_strict_json, parse_version_ack, VersionAck};
use crate::vcp_modules::sync::sync_error::{parse_wire_sync_error_frame, WireSyncError};
use serde_json::Value;

/// A fully validated frame accepted while the version handshake is pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionHandshakeFrame {
    VersionAck(VersionAck),
    SyncError(WireSyncError),
    SyncLogEvent,
}

/// Parses exactly one handshake frame and rejects every other message shape.
pub fn parse_version_handshake_json(input: &str) -> Result<VersionHandshakeFrame, String> {
    let payload =
        parse_strict_json(input).map_err(|error| format!("invalid handshake JSON: {error}"))?;
    parse_version_handshake_value(&payload)
}

fn parse_version_handshake_value(payload: &Value) -> Result<VersionHandshakeFrame, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "handshake frame must be an object".to_string())?;
    let message_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "handshake frame requires string type".to_string())?;

    match message_type {
        "VERSION_ACK" => parse_version_ack(payload)
            .map(VersionHandshakeFrame::VersionAck)
            .map_err(|error| error.to_string()),
        "SYNC_ERROR" => parse_wire_sync_error_frame(payload).map(VersionHandshakeFrame::SyncError),
        "SYNC_LOG_EVENT" => Ok(VersionHandshakeFrame::SyncLogEvent),
        other => Err(format!("unexpected handshake frame type {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_version_handshake_json, VersionHandshakeFrame};

    #[test]
    fn accepts_exact_ack_and_structured_error() {
        let ack = parse_version_handshake_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.2.0","protocolVersion":"1.2"}"#,
        )
        .expect("ack");
        assert!(matches!(ack, VersionHandshakeFrame::VersionAck(_)));

        let error = parse_version_handshake_json(
            r#"{"type":"SYNC_ERROR","error":{"code":"PLUGIN_VERSION_MISMATCH","origin":"desktop_plugin","stage":"handshake","kind":"compatibility","retry":"after_user_action","message":"mismatch","failedTopicIds":[]}}"#,
        )
        .expect("structured error");
        assert!(matches!(error, VersionHandshakeFrame::SyncError(_)));

        let unknown_stable_error = parse_version_handshake_json(
            r#"{"type":"SYNC_ERROR","error":{"code":"UPSTREAM_EXTENSION_FAILED","origin":"desktop_plugin","stage":"finalize","kind":"internal","retry":"manual","message":"extension failed","failedTopicIds":[]}}"#,
        )
        .expect("unknown stable Wire code");
        assert!(matches!(
            unknown_stable_error,
            VersionHandshakeFrame::SyncError(_)
        ));

        let log_event = parse_version_handshake_json(
            r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":1}"#,
        )
        .expect("Linux may broadcast a log event before VERSION_ACK");
        assert_eq!(log_event, VersionHandshakeFrame::SyncLogEvent);
    }

    #[test]
    fn rejects_legacy_duplicate_and_extended_error_frames() {
        for payload in [
            r#"{"type":"SYNC_ERROR","error":"legacy"}"#,
            r#"{"type":"SYNC_ERROR","error":{"code":"EAI_AGAIN","origin":"desktop_plugin","stage":"connect","kind":"connection","retry":"manual","message":"dns","failedTopicIds":[]}}"#,
            r#"{"type":"SYNC_ERROR","error":{"code":"PLUGIN_VERSION_MISMATCH","origin":"desktop_plugin","stage":"handshake","kind":"compatibility","retry":"after_user_action","message":"mismatch","failedTopicIds":[]},"debug":true}"#,
            r#"{"type":"SYNC_ERROR","type":"VERSION_ACK","error":{}}"#,
            r#"{"type":"PHASE_ACK","phase":"owner_metadata"}"#,
        ] {
            assert!(parse_version_handshake_json(payload).is_err());
        }
    }
}
