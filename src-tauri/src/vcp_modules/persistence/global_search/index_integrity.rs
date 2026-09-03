#[path = "rebuild.rs"]
mod rebuild;
#[path = "status.rs"]
mod status;

#[cfg(test)]
#[path = "index_integrity_tests.rs"]
mod tests;

use std::sync::OnceLock;
use tokio::sync::Mutex;

pub(crate) use rebuild::{rebuild_messages_fts, FtsRebuildResult};
pub(crate) use status::{get_fts_index_status, FtsIndexStatus};

static FTS_REBUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) fn rebuild_lock() -> &'static Mutex<()> {
    FTS_REBUILD_LOCK.get_or_init(|| Mutex::new(()))
}
