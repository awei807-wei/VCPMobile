//! Wire 1.6 / 1.5 握手与严格 JSON 契约。

mod diagnostic_frame;
pub mod handshake;
mod handshake_frame;
pub mod strict_json;

pub(crate) use diagnostic_frame::{parse_desktop_diagnostic_frame, DesktopDiagnosticFrame};
#[allow(unused_imports)]
pub use handshake::{build_version_check_json, build_version_check_json_for, WireProtocolVersion};
pub(crate) use handshake::{
    parse_version_ack_for, DesktopBackendMode, VersionAck, VersionAckError,
};
#[allow(unused_imports)]
pub use handshake_frame::{
    parse_version_handshake_json, parse_version_handshake_json_for, VersionHandshakeFrame,
};
pub use strict_json::parse_strict_json;

#[cfg(test)]
mod fixture_tests;
