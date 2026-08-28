//! Wire 1.2 握手与严格 JSON 契约。

pub mod handshake;
mod handshake_frame;
pub mod strict_json;

pub use handshake::build_version_check_json;
pub(crate) use handshake::{parse_version_ack, VersionAck};
pub use handshake_frame::{parse_version_handshake_json, VersionHandshakeFrame};
pub use strict_json::parse_strict_json;
