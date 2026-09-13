//! Wire 1.5 握手阶段允许帧的严格分派。

use super::{
    parse_desktop_diagnostic_frame, parse_strict_json, parse_version_ack, DesktopDiagnosticFrame,
    VersionAck, VersionAckError,
};
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
pub fn parse_version_handshake_json(input: &str) -> Result<VersionHandshakeFrame, VersionAckError> {
    let payload = parse_strict_json(input)
        .map_err(|error| VersionAckError::InvalidJson(error.to_string()))?;
    parse_version_handshake_value(&payload)
}

fn parse_version_handshake_value(
    payload: &Value,
) -> Result<VersionHandshakeFrame, VersionAckError> {
    let object = payload
        .as_object()
        .ok_or_else(|| VersionAckError::Invalid("handshake frame must be an object".to_string()))?;
    let message_type = object.get("type").and_then(Value::as_str).ok_or_else(|| {
        VersionAckError::Invalid("handshake frame requires string type".to_string())
    })?;

    match message_type {
        "VERSION_ACK" => parse_version_ack(payload).map(VersionHandshakeFrame::VersionAck),
        "SYNC_ERROR" => parse_wire_sync_error_frame(payload)
            .map(VersionHandshakeFrame::SyncError)
            .map_err(VersionAckError::Invalid),
        "SYNC_LOG_EVENT" => {
            match parse_desktop_diagnostic_frame(payload).map_err(VersionAckError::Invalid)? {
                DesktopDiagnosticFrame::SyncLog { .. } => Ok(VersionHandshakeFrame::SyncLogEvent),
                _ => Err(VersionAckError::Invalid(
                    "only SYNC_LOG_EVENT diagnostics are valid during handshake".to_string(),
                )),
            }
        }
        other => Err(VersionAckError::Invalid(format!(
            "unexpected handshake frame type {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_version_handshake_json, VersionHandshakeFrame};

    #[test]
    fn accepts_exact_ack_and_structured_error() {
        let ack = parse_version_handshake_json(
            r#"{"type":"VERSION_ACK","versions":[{"component":"desktop_plugin","version":"2.0.0"},{"component":"wire","version":"1.5"}],"backendMode":"cds"}"#,
        )
        .expect("ack");
        assert!(matches!(ack, VersionHandshakeFrame::VersionAck(_)));

        let error = parse_version_handshake_json(
            r#"{"type":"SYNC_ERROR","error":{"code":"WIRE_VERSION_MISMATCH","origin":"desktop_plugin","stage":"handshake","kind":"compatibility","retry":"after_user_action","message":"mismatch","failedTopicIds":[]}}"#,
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
            r#"{"type":"SYNC_ERROR","error":{"code":"WIRE_VERSION_MISMATCH","origin":"desktop_plugin","stage":"handshake","kind":"compatibility","retry":"after_user_action","message":"mismatch","failedTopicIds":[]},"debug":true}"#,
            r#"{"type":"SYNC_ERROR","type":"VERSION_ACK","error":{}}"#,
            r#"{"type":"PHASE_ACK","phase":"owner_metadata"}"#,
        ] {
            assert!(parse_version_handshake_json(payload).is_err());
        }
    }
}
