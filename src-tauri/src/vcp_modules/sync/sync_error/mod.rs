//! Wire 1.2 结构化同步错误契约。

mod codec;
mod payload;
mod registry;
mod registry_contexts;
mod registry_semantics;
mod types;
mod validation;

#[allow(unused_imports)]
pub use codec::{
    decode_wire_sync_error, encode_http_sync_error_body, encode_wire_sync_error,
    encode_wire_sync_error_value, parse_wire_sync_error_frame, WIRE_ERROR_MARKER,
};
pub use payload::{build_local_error_payload, build_wire_error_payload};
#[allow(unused_imports)]
pub use types::{
    SyncErrorCategory, SyncErrorOrigin, SyncErrorPayload, SyncErrorStage, SyncRetryAction,
    WireSyncError,
};
pub use validation::parse_wire_sync_error;

#[cfg(test)]
mod tests;
