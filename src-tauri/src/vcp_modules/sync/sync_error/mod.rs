//! Wire 1.2 结构化同步错误契约。

mod codec;
mod payload;
mod registry;
mod registry_contexts;
mod registry_semantics;
mod types;
mod validation;

pub use codec::parse_wire_sync_error_frame;
pub use types::WireSyncError;

#[cfg(test)]
mod tests;
