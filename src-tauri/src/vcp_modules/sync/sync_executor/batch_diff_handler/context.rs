use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_service::{
    Phase3DiffBatch, Phase3MessageSnapshots, Phase3Tracker, SyncCommand,
};
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

pub(crate) struct BatchContext<'a> {
    pub(crate) app: &'a AppHandle,
    pub(crate) client: &'a reqwest::Client,
    pub(crate) base_url: &'a str,
    pub(crate) token: &'a str,
    pub(crate) tracker: &'a Arc<Phase3Tracker>,
    pub(crate) tx: &'a mpsc::UnboundedSender<SyncCommand>,
    pub(crate) logger: &'a Arc<Mutex<SyncLogger>>,
    pub(crate) write_queue: &'a Arc<DbWriteQueue>,
    pub(crate) pending_batches: &'a Arc<AsyncMutex<VecDeque<Phase3DiffBatch>>>,
    pub(crate) prerender_enabled: bool,
    pub(crate) expected_topics: &'a Arc<AsyncMutex<HashSet<TopicKey>>>,
    pub(crate) expected_states: &'a Arc<AsyncMutex<Phase3MessageSnapshots>>,
    pub(crate) attempt_id: u64,
}
