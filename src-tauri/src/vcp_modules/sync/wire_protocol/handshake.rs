//! Wire 1.6 / 1.5 版本声明与握手确认契约。

use super::strict_json::parse_strict_json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

/// 移动端与桌面同步插件之间支持的 Wire 协议版本（首选 1.6，兼容回退 1.5）。
#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum WireProtocolVersion {
    #[serde(rename = "1.5")]
    V1_5,
    #[default]
    #[serde(rename = "1.6")]
    V1_6,
}

impl WireProtocolVersion {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1_5 => "1.5",
            Self::V1_6 => "1.6",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "1.5" => Some(Self::V1_5),
            "1.6" => Some(Self::V1_6),
            _ => None,
        }
    }
}

impl fmt::Display for WireProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 移动端默认首选的 Wire 版本。
pub const WIRE_PROTOCOL_VERSION: &str = WireProtocolVersion::V1_6.as_str();
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

/// 已通过 Wire 握手严格校验的桌面端声明。
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VersionAck {
    pub package_version: String,
    pub wire_version: String,
    pub backend_mode: DesktopBackendMode,
}

impl VersionAck {
    pub fn protocol_version(&self) -> Option<WireProtocolVersion> {
        WireProtocolVersion::parse(&self.wire_version)
    }
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

/// 构造默认首选版本（Wire 1.6）的移动端 `VERSION_CHECK` 对象。
pub fn build_version_check(mobile_version: &str) -> Result<Value, VersionAckError> {
    build_version_check_for(mobile_version, WireProtocolVersion::V1_6)
}

/// 构造指定 Wire 版本的移动端 `VERSION_CHECK` 对象。
pub fn build_version_check_for(
    mobile_version: &str,
    wire: WireProtocolVersion,
) -> Result<Value, VersionAckError> {
    validate_version_token(mobile_version, "mobile_app").map_err(VersionAckError::Invalid)?;
    let wire_str = wire.as_str();
    validate_version_token(wire_str, "wire").map_err(VersionAckError::Invalid)?;
    serde_json::to_value(VersionCheckFrame {
        frame_type: "VERSION_CHECK",
        versions: vec![
            VersionClaim {
                component: VersionComponent::MobileApp,
                version: mobile_version.to_string(),
            },
            VersionClaim {
                component: VersionComponent::Wire,
                version: wire_str.to_string(),
            },
        ],
    })
    .map_err(|error| {
        VersionAckError::Invalid(format!("VERSION_CHECK serialization failed: {error}"))
    })
}

/// 构造默认首选版本（Wire 1.6）序列化的移动端 `VERSION_CHECK` 帧。
pub fn build_version_check_json(mobile_version: &str) -> Result<String, VersionAckError> {
    build_version_check_json_for(mobile_version, WireProtocolVersion::V1_6)
}

/// 构造指定 Wire 版本序列化的移动端 `VERSION_CHECK` 帧。
pub fn build_version_check_json_for(
    mobile_version: &str,
    wire: WireProtocolVersion,
) -> Result<String, VersionAckError> {
    build_version_check_for(mobile_version, wire).map(|value| value.to_string())
}

/// 解析并严格校验序列化的桌面端 `VERSION_ACK` 帧（默认期望 Wire 1.6）。
#[allow(dead_code)]
pub fn parse_version_ack_json(input: &str) -> Result<VersionAck, VersionAckError> {
    parse_version_ack_json_for(input, WireProtocolVersion::V1_6)
}

/// 解析并严格校验序列化的桌面端 `VERSION_ACK` 帧（按指定期望 Wire 版本）。
#[allow(dead_code)]
pub fn parse_version_ack_json_for(
    input: &str,
    expected_wire: WireProtocolVersion,
) -> Result<VersionAck, VersionAckError> {
    let payload = parse_strict_json(input)
        .map_err(|error| VersionAckError::InvalidJson(error.to_string()))?;
    parse_version_ack_for(&payload, expected_wire)
}

/// 严格校验已解码的桌面端 `VERSION_ACK` 对象（默认期望 Wire 1.6）。
#[allow(dead_code)]
pub fn parse_version_ack(payload: &Value) -> Result<VersionAck, VersionAckError> {
    parse_version_ack_for(payload, WireProtocolVersion::V1_6)
}

/// 严格校验已解码的桌面端 `VERSION_ACK` 对象（按指定期望 Wire 版本）。
pub fn parse_version_ack_for(
    payload: &Value,
    expected_wire: WireProtocolVersion,
) -> Result<VersionAck, VersionAckError> {
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

    let expected_str = expected_wire.as_str();
    if wire_version != expected_str {
        return Err(VersionAckError::WireVersionMismatch {
            expected: expected_str.to_string(),
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
        build_version_check, build_version_check_for, parse_version_ack_json,
        parse_version_ack_json_for, DesktopBackendMode, VersionAckError, WireProtocolVersion,
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
    fn version_check_matches_wire_1_6_fixture() {
        let actual = build_version_check("1.1.6").expect("valid mobile version");
        let fixture: Value =
            serde_json::from_str(include_str!("../fixtures/version_handshake_contract.json"))
                .expect("version handshake fixture");
        assert_eq!(actual, fixture["versionCheck"]);
        assert_eq!(fixture["wireVersion"], WIRE_PROTOCOL_VERSION);
    }

    #[test]
    fn version_check_and_ack_support_wire_1_5_fallback() {
        let check = build_version_check_for("1.1.6", WireProtocolVersion::V1_5)
            .expect("valid legacy check");
        assert_eq!(
            check,
            json!({
                "type": "VERSION_CHECK",
                "versions": [
                    {"component": "mobile_app", "version": "1.1.6"},
                    {"component": "wire", "version": "1.5"}
                ]
            })
        );
        let ack = parse_version_ack_json_for(
            &valid_ack(json!([
                {"component": "desktop_plugin", "version": "2.0.0"},
                {"component": "wire", "version": "1.5"}
            ])),
            WireProtocolVersion::V1_5,
        )
        .expect("compatible 1.5 ACK");
        assert_eq!(ack.protocol_version(), Some(WireProtocolVersion::V1_5));
    }

    #[test]
    fn version_ack_is_order_independent_and_package_version_is_diagnostic() {
        for versions in [
            json!([
                {"component": "desktop_plugin", "version": "2.0.0"},
                {"component": "wire", "version": "1.6"}
            ]),
            json!([
                {"component": "wire", "version": "1.6"},
                {"component": "desktop_plugin", "version": "9.9.9"}
            ]),
        ] {
            let accepted = parse_version_ack_json(&valid_ack(versions)).expect("compatible ACK");
            assert_eq!(accepted.wire_version, WIRE_PROTOCOL_VERSION);
            assert_eq!(accepted.protocol_version(), Some(WireProtocolVersion::V1_6));
            assert_eq!(accepted.backend_mode, DesktopBackendMode::Cds);
        }
    }

    #[test]
    fn invalid_claim_sets_fail_before_wire_comparison() {
        let invalid = [
            json!([{"component": "wire", "version": "1.4"}]),
            json!([
                {"component": "wire", "version": "1.4"},
                {"component": "wire", "version": "1.6"}
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
            {"component": "wire", "version": "1.5"}
        ])))
        .expect_err("wire mismatch");
        assert_eq!(
            error,
            VersionAckError::WireVersionMismatch {
                expected: "1.6".to_string(),
                received: "1.5".to_string(),
                package_version: "2.0.0".to_string(),
            }
        );
    }

    #[test]
    fn old_fields_unknown_modes_and_duplicate_keys_are_rejected() {
        assert!(parse_version_ack_json(
            r#"{"type":"VERSION_ACK","pluginVersion":"2.0.0","protocolVersion":"1.6","backendMode":"cds"}"#
        )
        .is_err());
        assert!(parse_version_ack_json(
            r#"{"type":"VERSION_ACK","versions":[{"component":"desktop_plugin","version":"2.0.0"},{"component":"wire","version":"1.6"}],"backendMode":"fallback"}"#
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
