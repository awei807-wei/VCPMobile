mod attachment_canonicalization;
mod avatar_pulls;
mod batch_orchestration;
mod entity_batch;
mod entity_pulls;
mod frame_validation;
mod message_persistence;
mod ndjson_codec;
mod ndjson_stream;
mod result_reporting;

pub use batch_orchestration::PullBatchContext;

const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
const MAX_NDJSON_TRANSPORT_CHUNK_BYTES: usize = MAX_NDJSON_LINE_BYTES;
const MAX_NDJSON_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_NDJSON_ENTITIES: usize = 100_000;
const MAX_WARNING_SAMPLES: usize = 8;
const NDJSON_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PULL_WORKER_BUDGET_UNIT_BYTES: usize = 1024 * 1024;
const PULL_WORKER_BUDGET_UNITS: usize = MAX_NDJSON_LINE_BYTES / PULL_WORKER_BUDGET_UNIT_BYTES;
const MAX_ENTITY_BATCH_ITEMS: usize = 10_000;
const MAX_MESSAGE_PULL_TOPICS: usize = 10_000;
const SQLITE_BIND_CHUNK: usize = 400;
const MAX_ERROR_RESPONSE_BYTES: usize = 1024 * 1024;

/// Batch result for one pulled topic.
#[allow(dead_code)]
pub struct BatchPullResult {
    pub topic_id: String,
    pub success: bool,
    pub parsed_count: usize,
    pub failed_count: usize,
    pub legacy_attachment_warnings: usize,
    pub error: Option<String>,
}

pub struct PullExecutor;

/// Display-only progress context emitted while the NDJSON stream is processed.
#[derive(Clone)]
pub struct PullProgressContext {
    pub session_id: u64,
    pub base_completed: usize,
    pub total: usize,
    pub failed: usize,
    pub legacy_attachment_warnings: usize,
}

#[cfg(test)]
#[path = "pull_executor_tests.rs"]
mod tests;
