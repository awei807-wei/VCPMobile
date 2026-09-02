//! Strict validation for Linux desktop diagnostic frames on Wire 1.2.

use serde_json::{Map, Value};

const MAX_DIAGNOSTIC_PHASE_CHARS: usize = 64;
const MAX_DIAGNOSTIC_MESSAGE_CHARS: usize = 4_096;
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DesktopDiagnosticFrame {
    SyncLog {
        level: String,
        phase: String,
        message: String,
    },
    PhaseStart {
        phase: String,
    },
    PhaseComplete {
        phase: String,
    },
}

pub(crate) fn parse_desktop_diagnostic_frame(
    payload: &Value,
) -> Result<DesktopDiagnosticFrame, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "desktop diagnostic frame must be an object".to_string())?;
    let frame_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "desktop diagnostic frame requires string type".to_string())?;

    match frame_type {
        "SYNC_LOG_EVENT" => parse_log_event(object),
        "DESKTOP_PHASE_START" => parse_phase_event(object, true),
        "DESKTOP_PHASE_COMPLETE" => parse_phase_event(object, false),
        other => Err(format!("unknown desktop diagnostic frame type {other}")),
    }
}

fn parse_log_event(object: &Map<String, Value>) -> Result<DesktopDiagnosticFrame, String> {
    require_exact_keys(object, &["type", "level", "phase", "message", "ts"])?;
    let level = require_text(object, "level", 8)?;
    if !matches!(level.as_str(), "info" | "warn" | "error") {
        return Err("SYNC_LOG_EVENT.level must be info, warn, or error".to_string());
    }
    let phase = require_text(object, "phase", MAX_DIAGNOSTIC_PHASE_CHARS)?;
    let message = require_text(object, "message", MAX_DIAGNOSTIC_MESSAGE_CHARS)?;
    require_timestamp(object)?;
    Ok(DesktopDiagnosticFrame::SyncLog {
        level,
        phase,
        message,
    })
}

fn parse_phase_event(
    object: &Map<String, Value>,
    is_start: bool,
) -> Result<DesktopDiagnosticFrame, String> {
    require_exact_keys(object, &["type", "phase", "ts"])?;
    let phase = require_text(object, "phase", MAX_DIAGNOSTIC_PHASE_CHARS)?;
    require_timestamp(object)?;
    if is_start {
        Ok(DesktopDiagnosticFrame::PhaseStart { phase })
    } else {
        Ok(DesktopDiagnosticFrame::PhaseComplete { phase })
    }
}

fn require_exact_keys(object: &Map<String, Value>, expected: &[&str]) -> Result<(), String> {
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(format!(
            "desktop diagnostic frame must contain exactly {}",
            expected.join(", ")
        ));
    }
    Ok(())
}

fn require_text(
    object: &Map<String, Value>,
    field: &str,
    max_chars: usize,
) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.chars().count() <= max_chars
                && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| {
            format!(
                "desktop diagnostic frame {field} must be non-empty, bounded text without controls"
            )
        })?;
    Ok(value.to_string())
}

fn require_timestamp(object: &Map<String, Value>) -> Result<(), String> {
    let valid = object
        .get("ts")
        .and_then(Value::as_u64)
        .is_some_and(|value| value <= MAX_SAFE_JSON_INTEGER);
    if valid {
        Ok(())
    } else {
        Err("desktop diagnostic frame ts must be a non-negative safe integer".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcp_modules::sync::wire_protocol::parse_strict_json;

    fn parse(input: &str) -> Result<DesktopDiagnosticFrame, String> {
        let value = parse_strict_json(input).map_err(|error| error.to_string())?;
        parse_desktop_diagnostic_frame(&value)
    }

    #[test]
    fn accepts_only_current_linux_diagnostic_shapes() {
        assert!(matches!(
            parse(
                r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":1}"#
            ),
            Ok(DesktopDiagnosticFrame::SyncLog { .. })
        ));
        assert!(matches!(
            parse(r#"{"type":"DESKTOP_PHASE_START","phase":"owner_metadata","ts":2}"#),
            Ok(DesktopDiagnosticFrame::PhaseStart { .. })
        ));
        assert!(matches!(
            parse(r#"{"type":"DESKTOP_PHASE_COMPLETE","phase":"messages","ts":3}"#),
            Ok(DesktopDiagnosticFrame::PhaseComplete { .. })
        ));
    }

    #[test]
    fn rejects_extensions_legacy_progress_and_unsafe_fields() {
        for input in [
            r#"{"type":"SYNC_LOG_EVENT","level":"debug","phase":"websocket","message":"connected","ts":1}"#,
            r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":1,"debug":true}"#,
            r#"{"type":"SYNC_LOG_EVENT","level":"info","phase":"websocket","message":"connected","ts":-1}"#,
            r#"{"type":"DESKTOP_PHASE_PROGRESS","phase":"messages","ts":1}"#,
            r#"{"type":"DESKTOP_PHASE_START","phase":"messages\nspoof","ts":1}"#,
            r#"{"type":"DESKTOP_PHASE_START","type":"DESKTOP_PHASE_COMPLETE","phase":"messages","ts":1}"#,
        ] {
            assert!(parse(input).is_err(), "invalid frame accepted: {input}");
        }
    }
}
