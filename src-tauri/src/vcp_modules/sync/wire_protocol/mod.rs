//! Wire 1.4 握手与严格 JSON 契约。

mod diagnostic_frame;
pub mod handshake;
mod handshake_frame;
pub mod strict_json;

pub(crate) use diagnostic_frame::{parse_desktop_diagnostic_frame, DesktopDiagnosticFrame};
pub use handshake::build_version_check_json;
pub(crate) use handshake::{parse_version_ack, VersionAck};
pub use handshake_frame::{parse_version_handshake_json, VersionHandshakeFrame};
pub use strict_json::parse_strict_json;

#[cfg(test)]
mod fixture_tests;
