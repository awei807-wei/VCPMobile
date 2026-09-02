//! Wire 1.2 mobile sync service façade.
//!
//! Runtime state, protocol framing, lifecycle, phase orchestration and log
//! commands live in focused child modules. This file intentionally contains
//! only the stable API surface consumed by Tauri and the rest of VCPMobile.

mod attempt;
mod batching;
mod commands;
mod diagnostics;
mod entity;
mod errors;
mod frames;
mod lifecycle;
mod logs;
mod phase;
mod protocol;
mod session;
mod session_support;
mod types;

#[allow(unused_imports)]
pub use diagnostics::ConnectionErrorDiagnosis;
pub use lifecycle::{
    get_sync_status, init_sync_service, is_sync_active, start_manual_sync, stop_sync,
};
#[allow(unused_imports)]
pub use logs::{
    clear_old_sync_logs, get_sync_session_log_path, list_sync_log_files, read_sync_log_file,
    SyncLogCleanupResult, SyncLogFileInfo,
};
#[allow(unused_imports)]
pub use types::{NetworkAwareSemaphore, Phase3Tracker, SyncCommand, SyncCommandRouter, SyncState};

pub(crate) use logs::emit_sync_log;
pub(crate) use types::SyncTaskTracker;

#[cfg(test)]
mod tests;
