use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{Phase3Tracker, SyncCommand};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock};

pub(crate) struct Phase3ContextParams<'a> {
    pub(crate) app_handle: &'a AppHandle,
    pub(crate) http_client: &'a reqwest::Client,
    pub(crate) base_url: &'a str,
    pub(crate) token: &'a str,
    pub(crate) tracker: &'a Arc<Phase3Tracker>,
    pub(crate) tx_internal: &'a mpsc::UnboundedSender<SyncCommand>,
    pub(crate) logger: &'a Arc<Mutex<SyncLogger>>,
    pub(crate) write_queue: &'a Arc<DbWriteQueue>,
    pub(crate) pending_diff_batches:
        &'a Arc<AsyncMutex<VecDeque<serde_json::Map<String, serde_json::Value>>>>,
    pub(crate) prerender_enabled: bool,
    pub(crate) uploaded_hashes: &'a Arc<RwLock<HashSet<String>>>,
    pub(crate) expected_batch_topics: &'a Arc<AsyncMutex<HashSet<String>>>,
    pub(crate) attempt_id: u64,
}

pub struct Phase3Context {
    pub(crate) app_handle: AppHandle,
    pub(crate) http_client: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) token: String,
    pub(crate) tracker: Arc<Phase3Tracker>,
    pub(crate) tx_internal: mpsc::UnboundedSender<SyncCommand>,
    pub(crate) logger: Arc<Mutex<SyncLogger>>,
    pub(crate) write_queue: Arc<DbWriteQueue>,
    pub(crate) pending_diff_batches:
        Arc<AsyncMutex<VecDeque<serde_json::Map<String, serde_json::Value>>>>,
    pub(crate) prerender_enabled: bool,
    pub(crate) uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub(crate) expected_batch_topics: Arc<AsyncMutex<HashSet<String>>>,
    pub(crate) attempt_id: u64,
}

impl Phase3Context {
    pub(crate) fn new(params: Phase3ContextParams<'_>) -> Self {
        Self {
            app_handle: params.app_handle.clone(),
            http_client: params.http_client.clone(),
            base_url: params.base_url.to_string(),
            token: params.token.to_string(),
            tracker: params.tracker.clone(),
            tx_internal: params.tx_internal.clone(),
            logger: params.logger.clone(),
            write_queue: params.write_queue.clone(),
            pending_diff_batches: params.pending_diff_batches.clone(),
            prerender_enabled: params.prerender_enabled,
            uploaded_hashes: params.uploaded_hashes.clone(),
            expected_batch_topics: params.expected_batch_topics.clone(),
            attempt_id: params.attempt_id,
        }
    }
}
