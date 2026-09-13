//! Wire 1.5 版本声明与握手确认契约。

use super::strict_json::parse_strict_json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// 移动端与桌面同步插件之间唯一允许的 Wire 版本。
pub const WIRE_PROTOCOL_VERSION: &str = "1.5";
const MAX_VERSION_TOKEN_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VersionComponent {
    MobileApp,
    DesktopPlugin,
    Wire,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionClaim {
    component: VersionComponent,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionCheckFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    versions: Vec<VersionClaim>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionAckFrame {
    #[serde(rename = "type")]
    frame_type: String,
    versions: Vec<VersionClaim>,
    backend_mode: DesktopBackendMode,
}

/// 桌面同步插件实际使用的数据后端。
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopBackendMode {
    Legacy,
    Cds,
}

/// 已通过 Wire 1.5 严格校验的桌面端声明。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VersionAck {
    pub package_version: String,
    pub wire_version: String,
    pub backend_mode: DesktopBackendMode,
}

/// 版本声明校验失败。
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum VersionAckError {
    InvalidJson(String),
    Invalid(String),
    WireVersionMismatch {
        expected: String,
        received: String,
        package_version: String,
    },
}

impl fmt::Display for VersionAckError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid handshake JSON: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::WireVersionMismatch {
                expected, received, ..
            } => write!(
                formatter,
                "wire protocol mismatch: expected {expected}, received {received}"
            ),
        }
    }
}

impl std::error::Error for VersionAckError {}

/// 构造精确的移动端 `VERSION_CHECK` 对象。
pub fn build_version_check(mobile_version: &str) -> Result<Value, VersionAckError> {
    validate_version_token(mobile_version, "mobile_app").map_err(VersionAckError::Invalid)?;
    validate_version_token(WIRE_PROTOCOL_VERSION, "wire").map_err(VersionAckError::Invalid)?;
    serde_json::to_value(VersionCheckFrame {
        frame_type: "VERSION_CHECK",
        versions: vec![
            VersionClaim {
                component: VersionComponent::MobileApp,
                version: mobile_version.to_string(),
            },
            VersionClaim {
                component: VersionComponent::Wire,
                version: WIRE_PROTOCOL_VERSION.to_string(),
            },
        ],
    })
    .map_err(|error| {
        VersionAckError::Invalid(format!("VERSION_CHECK serialization failed: {error}"))
    })
}

/// 构造精确序列化的移动端 `VERSION_CHECK` 帧。
pub fn build_version_check_json(mobile_version: &str) -> Result<String, VersionAckError> {
    build_version_check(mobile_version).map(|value| value.to_string())
}

/// 解析并严格校验序列化的桌面端 `VERSION_ACK` 帧。
pub fn parse_version_ack_json(input: &str) -> Result<VersionAck, VersionAckError> {
    let payload = parse_strict_json(input)
        .map_err(|error| VersionAckError::InvalidJson(error.to_string()))?;
    parse_version_ack(&payload)
}

/// 严格校验已解码的桌面端 `VERSION_ACK` 对象。
pub fn parse_version_ack(payload: &Value) -> Result<VersionAck, VersionAckError> {
    let ack = serde_json::from_value::<VersionAckFrame>(payload.clone())
        .map_err(|error| VersionAckError::Invalid(format!("invalid VERSION_ACK: {error}")))?;
    if ack.frame_type != "VERSION_ACK" {
        return Err(VersionAckError::Invalid("expected VERSION_ACK".to_string()));
    }

    let mut versions = validate_claims(
        ack.versions,
        &[VersionComponent::DesktopPlugin, VersionComponent::Wire],
        "VERSION_ACK",
    )
    .map_err(VersionAckError::Invalid)?;
    let package_version = versions
        .remove(&VersionComponent::DesktopPlugin)
        .ok_or_else(|| {
            VersionAckError::Invalid("VERSION_ACK is missing desktop_plugin version".to_string())
        })?;
    let wire_version = versions.remove(&VersionComponent::Wire).ok_or_else(|| {
        VersionAckError::Invalid("VERSION_ACK is missing wire version".to_string())
    })?;

    if wire_version != WIRE_PROTOCOL_VERSION {
        return Err(VersionAckError::WireVersionMismatch {
            expected: WIRE_PROTOCOL_VERSION.to_string(),
            received: wire_version,
            package_version,
        });
    }

    Ok(VersionAck {
        package_version,
        wire_version,
        backend_mode: ack.backend_mode,
    })
}

fn validate_claims(
    claims: Vec<VersionClaim>,
    expected: &[VersionComponent],
    label: &str,
) -> Result<HashMap<VersionComponent, String>, String> {
    if claims.len() != expected.len() {
        return Err(format!(
            "{label}.versions must contain exactly {} entries",
            expected.len()
        ));
    }

    let mut versions = HashMap::with_capacity(expected.len());
    for claim in claims {
        if !expected.contains(&claim.component) {
            return Err(format!(
                "{label}.versions contains unexpected component {:?}",
                claim.component
            ));
        }
        validate_version_token(&claim.version, "version")?;
        if versions.insert(claim.component, claim.version).is_some() {
            return Err(format!(
                "{label}.versions contains duplicate component {:?}",
                claim.component
            ));
        }
    }

    if expected
        .iter()
        .any(|component| !versions.contains_key(component))
    {
        return Err(format!("{label}.versions is missing a required component"));
    }
    Ok(versions)
}

fn validate_version_token(value: &str, label: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_VERSION_TOKEN_BYTES {
        return Err(format!(
            "{label} version must contain 1 to {MAX_VERSION_TOKEN_BYTES} bytes"
        ));
    }
    if !bytes[0].is_ascii_alphanumeric()
        || bytes[1..].iter().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'+' | b'-')
        })
    {
        return Err(format!("{label} version contains unsafe characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_version_check, parse_version_ack_json, DesktopBackendMode, VersionAckError,
        WIRE_PROTOCOL_VERSION,
    };
    use serde_json::{json, Value};

    fn valid_ack(versions: Value) -> String {
        json!({
            "type": "VERSION_ACK",
            "versions": versions,
            "backendMode": "cds",
        })
        .to_string()
    }

    #[test]
    fn version_check_matches_wire_1_5_fixture() {
        let actual = build_version_check("1.1.6").expect("valid mobile version");
        let fixture: Value =
            serde_json::from_str(include_str!("../fixtures/version_handshake_contract.json"))
                .expect("version handshake fixture");
        assert_eq!(actual, fixture["versionCheck"]);
        assert_eq!(fixture["wireVersion"], WIRE_PROTOCOL_VERSION);
    }

    #[test]
    fn version_ack_is_order_independent_and_package_version_is_diagnostic() {
        for versions in [
            json!([
                {"component": "desktop_plugin", "version": "2.0.0"},
                {"component": "wire", "version": "1.5"}
            ]),
            json!([
                {"component": "wire", "version": "1.5"},
                {"component": "desktop_plugin", "version": "9.9.9"}
            ]),
        ] {
            let accepted = parse_version_ack_json(&valid_ack(versions)).expect("compatible ACK");
            assert_eq!(accepted.wire_version, WIRE_PROTOCOL_VERSION);
            assert_eq!(accepted.backend_mode, DesktopBackendMode::Cds);
        }
    }

    #[test]
    fn invalid_claim_sets_fail_before_wire_comparison() {
        let invalid = [
            json!([{"component": "wire", "version": "1.4"}]),
            json!([
                {"component": "wire", "version": "1.4"},
                {"component": "wire", "version": "1.5"}
            ]),
            json!([
                {"component": "mobile_app", "version": "1.1.6"},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": "bad version"},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": "1".repeat(65)},
                {"component": "wire", "version": "1.4"}
            ]),
        ];
        for versions in invalid {
            assert!(matches!(
                parse_version_ack_json(&valid_ack(versions)),
                Err(VersionAckError::Invalid(_))
            ));
        }
    }

    #[test]
    fn compatible_shape_with_wrong_wire_has_typed_mismatch() {
        let error = parse_version_ack_json(&valid_ack(json!([
            {"component": "desktop_plugin", "version": "2.0.0"},
            {"component": "wire", "version": "1.4"}
        ])))
        .expect_err("wire mismatch");
        assert_eq!(
            error,
            VersionAckError::WireVersionMismatch {
                expected: "1.5".to_string(),
                received: "1.4".to_string(),
                package_version: "2.0.0".to_string(),
            }
        );
    }

    #[test]
    fn old_fields_unknown_modes_and_duplicate_keys_are_rejected() {
        assert!(parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"2.0.0","protocolVersion":"1.5","backendMode":"cds"}"#
        )
        .is_err());
        assert!(parse_version_ack_json(
            r#"{"type":"VERSION_ACK","versions":[{"component":"desktop_plugin","version":"2.0.0"},{"component":"wire","version":"1.5"}],"backendMode":"fallback"}"#
        )
        .is_err());
        assert!(matches!(
            parse_version_ack_json(
                r#"{"type":"VERSION_ACK","type":"VERSION_ACK","versions":[],"backendMode":"cds"}"#
            ),
            Err(VersionAckError::InvalidJson(_))
        ));
    }
}
