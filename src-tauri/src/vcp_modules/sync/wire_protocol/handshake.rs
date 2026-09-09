//! Wire 1.4 version handshake contract.

use super::strict_json::parse_strict_json;
use serde_json::{json, Value};
use std::fmt;

/// The desktop plugin version currently used as the deployment baseline.
///
/// Wire compatibility is owned by [`WIRE_PROTOCOL_VERSION`].  The plugin
/// version is retained for diagnostics and baseline pinning; patch releases
/// that keep the same wire contract are not rejected by the ACK parser.
pub const EXPECTED_PLUGIN_VERSION: &str = "1.4.0";

/// The hard-cut wire protocol version used by the mobile sync service.
pub const WIRE_PROTOCOL_VERSION: &str = "1.4";

/// A validated desktop `VERSION_ACK` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionAck {
    pub plugin_version: String,
    pub protocol_version: String,
}

/// Failure while validating a `VERSION_ACK` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionAckError {
    InvalidJson(String),
    PayloadMustBeObject,
    UnknownField(String),
    MissingField(&'static str),
    InvalidFieldType(&'static str),
    InvalidFieldValue(&'static str),
    InvalidMessageType,
    /// Retained for callers that classify package diagnostics separately.
    ///
    /// The Wire 1.4 compatibility gate intentionally does not emit this
    /// variant: plugin patch versions are diagnostic, while the wire version
    /// is the hard compatibility boundary.
    PluginVersionMismatch {
        expected: String,
        received: String,
    },
    ProtocolVersionMismatch {
        expected: String,
        received: String,
    },
}

impl fmt::Display for VersionAckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid JSON: {error}"),
            Self::PayloadMustBeObject => formatter.write_str("VERSION_ACK must be an object"),
            Self::UnknownField(field) => write!(formatter, "unknown VERSION_ACK field: {field}"),
            Self::MissingField(field) => write!(formatter, "missing VERSION_ACK field: {field}"),
            Self::InvalidFieldType(field) => {
                write!(formatter, "VERSION_ACK.{field} must be a string")
            }
            Self::InvalidFieldValue(field) => {
                write!(formatter, "VERSION_ACK.{field} must be non-empty")
            }
            Self::InvalidMessageType => formatter.write_str("expected VERSION_ACK"),
            Self::PluginVersionMismatch { expected, received } => write!(
                formatter,
                "plugin version mismatch: expected {expected}, received {received}"
            ),
            Self::ProtocolVersionMismatch { expected, received } => write!(
                formatter,
                "wire protocol mismatch: expected {expected}, received {received}"
            ),
        }
    }
}

impl std::error::Error for VersionAckError {}

/// Builds the exact mobile-to-desktop `VERSION_CHECK` object.
pub fn build_version_check(mobile_version: &str) -> Value {
    json!({
        "type": "VERSION_CHECK",
        "mobileVersion": mobile_version,
        "protocolVersion": WIRE_PROTOCOL_VERSION,
    })
}

/// Builds the exact serialized mobile-to-desktop `VERSION_CHECK` frame.
pub fn build_version_check_json(mobile_version: &str) -> String {
    build_version_check(mobile_version).to_string()
}

/// Parses and strictly validates a serialized desktop `VERSION_ACK` frame.
pub fn parse_version_ack_json(input: &str) -> Result<VersionAck, VersionAckError> {
    let payload = parse_strict_json(input)
        .map_err(|error| VersionAckError::InvalidJson(error.to_string()))?;
    parse_version_ack(&payload)
}

/// Strictly validates a decoded desktop `VERSION_ACK` object.
pub fn parse_version_ack(payload: &Value) -> Result<VersionAck, VersionAckError> {
    let object = payload
        .as_object()
        .ok_or(VersionAckError::PayloadMustBeObject)?;

    for field in object.keys() {
        if !matches!(field.as_str(), "type" | "pluginVersion" | "protocolVersion") {
            return Err(VersionAckError::UnknownField(field.clone()));
        }
    }

    let message_type = object
        .get("type")
        .ok_or(VersionAckError::MissingField("type"))?
        .as_str()
        .ok_or(VersionAckError::InvalidFieldType("type"))?;
    if message_type != "VERSION_ACK" {
        return Err(VersionAckError::InvalidMessageType);
    }

    let plugin_version = required_nonempty_string(object, "pluginVersion")?;
    let protocol_version = required_nonempty_string(object, "protocolVersion")?;
    if protocol_version != WIRE_PROTOCOL_VERSION {
        return Err(VersionAckError::ProtocolVersionMismatch {
            expected: WIRE_PROTOCOL_VERSION.to_owned(),
            received: protocol_version.to_owned(),
        });
    }

    Ok(VersionAck {
        plugin_version: plugin_version.to_owned(),
        protocol_version: protocol_version.to_owned(),
    })
}

/// Alias for callers that distinguish decoded payload validation from JSON parsing.
pub fn validate_version_ack(payload: &Value) -> Result<VersionAck, VersionAckError> {
    parse_version_ack(payload)
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, VersionAckError> {
    object
        .get(field)
        .ok_or(VersionAckError::MissingField(field))?
        .as_str()
        .ok_or(VersionAckError::InvalidFieldType(field))
}

fn required_nonempty_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, VersionAckError> {
    let value = required_string(object, field)?;
    if value.is_empty() {
        return Err(VersionAckError::InvalidFieldValue(field));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{
        build_version_check, parse_version_ack, parse_version_ack_json, VersionAckError,
        EXPECTED_PLUGIN_VERSION, WIRE_PROTOCOL_VERSION,
    };
    use serde_json::json;

    #[test]
    fn version_constants_are_wire_1_4() {
        assert_eq!(EXPECTED_PLUGIN_VERSION, "1.4.0");
        assert_eq!(WIRE_PROTOCOL_VERSION, "1.4");
    }

    #[test]
    fn version_check_contains_protocol_version() {
        assert_eq!(
            build_version_check("1.1.4"),
            json!({
                "type": "VERSION_CHECK",
                "mobileVersion": "1.1.4",
                "protocolVersion": "1.4",
            })
        );
    }

    #[test]
    fn accepts_exact_version_ack() {
        let ack = parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":"1.4"}"#,
        )
        .expect("the exact Wire 1.4 ACK should pass");
        assert_eq!(ack.plugin_version, "1.4.0");
        assert_eq!(ack.protocol_version, "1.4");
    }

    #[test]
    fn rejects_unknown_ack_fields() {
        let error = parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":"1.4","version":"1.4.0"}"#,
        )
        .expect_err("unknown fields must fail closed");
        assert!(matches!(error, VersionAckError::UnknownField(field) if field == "version"));
    }

    #[test]
    fn rejects_missing_ack_fields() {
        let error = parse_version_ack_json(r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0"}"#)
            .expect_err("missing protocol version must fail closed");
        assert!(matches!(
            error,
            VersionAckError::MissingField("protocolVersion")
        ));
    }

    #[test]
    fn rejects_old_wire_versions() {
        let old_wire = parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.2.0","protocolVersion":"1.2"}"#,
        )
        .expect_err("old wire version must fail closed");
        assert!(matches!(
            old_wire,
            VersionAckError::ProtocolVersionMismatch { .. }
        ));
    }

    #[test]
    fn rejects_wrong_types_and_message_type() {
        let wrong_type = parse_version_ack(&json!({
            "type": "VERSION_CHECK",
            "pluginVersion": "1.4.0",
            "protocolVersion": "1.4",
        }))
        .expect_err("wrong message type must fail closed");
        assert_eq!(wrong_type, VersionAckError::InvalidMessageType);

        let wrong_field_type = parse_version_ack(&json!({
            "type": "VERSION_ACK",
            "pluginVersion": 120,
            "protocolVersion": "1.4",
        }))
        .expect_err("wrong field types must fail closed");
        assert_eq!(
            wrong_field_type,
            VersionAckError::InvalidFieldType("pluginVersion")
        );
    }

    #[test]
    fn rejects_duplicate_ack_keys_before_schema_validation() {
        let error = parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","pluginVersion":"1.4.0","protocolVersion":"1.4"}"#,
        )
        .expect_err("duplicate ACK keys must fail closed");
        assert!(matches!(error, VersionAckError::InvalidJson(_)));
    }

    #[test]
    fn accepts_a_patch_plugin_release_when_wire_is_compatible() {
        let ack = parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"1.4.9","protocolVersion":"1.4"}"#,
        )
        .expect("plugin patch releases do not change the wire contract");
        assert_eq!(ack.plugin_version, "1.4.9");
    }

    #[test]
    fn rejects_empty_ack_versions() {
        for field in ["pluginVersion", "protocolVersion"] {
            let input = if field == "pluginVersion" {
                r#"{"type":"VERSION_ACK","pluginVersion":"","protocolVersion":"1.4"}"#
            } else {
                r#"{"type":"VERSION_ACK","pluginVersion":"1.4.0","protocolVersion":""}"#
            };
            assert!(matches!(
                parse_version_ack_json(input),
                Err(VersionAckError::InvalidFieldValue(actual)) if actual == field
            ));
        }
    }
}
