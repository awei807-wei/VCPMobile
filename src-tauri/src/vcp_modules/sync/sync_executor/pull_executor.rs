mod avatar_pulls;
mod batch_orchestration;
mod entity_batch;
mod entity_pulls;
mod frame_validation;
mod message_persistence;
mod ndjson_codec;
mod ndjson_stream;
mod result_reporting;

pub(crate) const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_NDJSON_TOTAL_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const MAX_NDJSON_TOPICS: usize = 10_000;
pub(crate) const MAX_NDJSON_ENTITIES: usize = 100_000;
pub(crate) const MAX_WARNING_SAMPLES: usize = 8;
pub(crate) const NDJSON_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) const MAX_ENTITY_BATCH_ITEMS: usize = 1_000;
pub(crate) const MAX_MESSAGES_PER_TOPIC: usize = 10_000;
pub(crate) const MAX_ERROR_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_AVATAR_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
pub(crate) const SQLITE_BIND_CHUNK: usize = 400;

/// Batch result for one pulled topic.
#[allow(dead_code)]
pub struct BatchPullResult {
    pub topic: crate::vcp_modules::topic_types::TopicKey,
    pub success: bool,
    pub parsed_count: usize,
    pub failed_count: usize,
    pub legacy_attachment_warnings: usize,
    pub error: Option<String>,
}

pub struct PullExecutor;

/// Identity and optional manifest hash for one avatar pull.
#[derive(Debug, Clone, Copy)]
pub struct AvatarPullTarget<'a> {
    pub owner_type: &'a str,
    pub owner_id: &'a str,
    pub expected_hash: Option<&'a str>,
}

/// Shared request context for a Wire 1.4 message batch pull.
pub struct MessageBatchPullRequest<'a, R: tauri::Runtime> {
    pub app: &'a tauri::AppHandle<R>,
    pub client: &'a reqwest::Client,
    pub http_url: &'a str,
    pub sync_token: &'a str,
    pub requests: &'a [(crate::vcp_modules::topic_types::TopicKey, Vec<String>)],
    pub expected_local_states: Option<
        &'a std::collections::HashMap<
            crate::vcp_modules::topic_types::TopicKey,
            std::collections::BTreeMap<String, crate::vcp_modules::sync_types::MessageVersionState>,
        >,
    >,
    pub write_queue: &'a crate::vcp_modules::db_write_queue::DbWriteQueue,
    pub prerender_enabled: bool,
    pub progress: Option<PullProgressContext>,
}

/// Display-only progress context emitted while the NDJSON stream is processed.
#[derive(Clone)]
pub struct PullProgressContext {
    pub session_id: u64,
    pub attempt_id: u64,
    pub base_completed: usize,
    pub total: usize,
    pub failed: usize,
    pub legacy_attachment_warnings: usize,
}

#[cfg(test)]
#[path = "pull_executor_tests.rs"]
mod tests;
