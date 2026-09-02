use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{SyncCommand, SyncTaskTracker};
use crate::vcp_modules::sync_types::SyncDataType;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, AtomicU8};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub(crate) struct DiffContextParams<'a> {
    pub(crate) app_handle: &'a tauri::AppHandle,
    pub(crate) data_type: SyncDataType,
    pub(crate) http_client: &'a reqwest::Client,
    pub(crate) base_url: &'a str,
    pub(crate) token: &'a str,
    pub(crate) write_queue: &'a Arc<DbWriteQueue>,
    pub(crate) pending_tasks: &'a Arc<AtomicU32>,
    pub(crate) total_tasks: &'a Arc<AtomicU32>,
    pub(crate) manifest_responses_received: &'a Arc<AtomicU32>,
    pub(crate) expected_manifest_count: &'a Arc<AtomicU32>,
    pub(crate) expected_manifest_types: &'a Arc<Mutex<HashSet<String>>>,
    pub(crate) manifest_phase: &'a Arc<AtomicU8>,
    pub(crate) tx_internal: &'a mpsc::UnboundedSender<SyncCommand>,
    pub(crate) changed_owners: &'a Arc<tokio::sync::Mutex<HashSet<String>>>,
    pub(crate) logger: &'a Arc<Mutex<SyncLogger>>,
    pub(crate) task_tracker: &'a Arc<SyncTaskTracker>,
    pub(crate) session_id: u64,
    pub(crate) attempt_id: u64,
}

#[derive(Clone)]
pub struct DiffContext {
    pub(crate) app_handle: tauri::AppHandle,
    pub(crate) data_type: SyncDataType,
    pub(crate) http_client: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) token: String,
    pub(crate) write_queue: Arc<DbWriteQueue>,
    pub(crate) pending_tasks: Arc<AtomicU32>,
    pub(crate) total_tasks: Arc<AtomicU32>,
    pub(crate) manifest_responses_received: Arc<AtomicU32>,
    pub(crate) expected_manifest_count: Arc<AtomicU32>,
    pub(crate) expected_manifest_types: Arc<Mutex<HashSet<String>>>,
    pub(crate) manifest_phase: Arc<AtomicU8>,
    pub(crate) tx_internal: mpsc::UnboundedSender<SyncCommand>,
    pub(crate) changed_owners: Arc<tokio::sync::Mutex<HashSet<String>>>,
    pub(crate) logger: Arc<Mutex<SyncLogger>>,
    pub(crate) task_tracker: Arc<SyncTaskTracker>,
    pub(crate) session_id: u64,
    pub(crate) attempt_id: u64,
}

impl DiffContext {
    pub(crate) fn new(params: DiffContextParams<'_>) -> Self {
        Self {
            app_handle: params.app_handle.clone(),
            data_type: params.data_type,
            http_client: params.http_client.clone(),
            base_url: params.base_url.to_string(),
            token: params.token.to_string(),
            write_queue: params.write_queue.clone(),
            pending_tasks: params.pending_tasks.clone(),
            total_tasks: params.total_tasks.clone(),
            manifest_responses_received: params.manifest_responses_received.clone(),
            expected_manifest_count: params.expected_manifest_count.clone(),
            expected_manifest_types: params.expected_manifest_types.clone(),
            manifest_phase: params.manifest_phase.clone(),
            tx_internal: params.tx_internal.clone(),
            changed_owners: params.changed_owners.clone(),
            logger: params.logger.clone(),
            task_tracker: params.task_tracker.clone(),
            session_id: params.session_id,
            attempt_id: params.attempt_id,
        }
    }
}
