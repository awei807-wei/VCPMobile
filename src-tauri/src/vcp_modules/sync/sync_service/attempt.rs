use super::commands;
use super::frames;
use super::phase;
use super::types::{
    MessagePhaseBarrier, NetworkAwareSemaphore, PendingDiffBatch, PendingFinalAck, Phase3Tracker,
    SyncCommand, SyncTaskTracker, SyncWebSocket,
};
use super::Phase3MessageSnapshots;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_pipeline::pipeline::{PipelineCommand, SyncPipeline};
use crate::vcp_modules::sync_types::ManifestType;
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8};
use std::sync::{Arc, Mutex};
use tauri::AppHandle;
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock};
use tokio::time::{interval, Interval};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptAction {
    Continue,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptOutcome {
    pub(crate) success: bool,
    pub(crate) fatal: bool,
    pub(crate) retry: Option<RetryReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetryReason {
    pub(crate) code: String,
    pub(crate) message: String,
}

pub(crate) struct AttemptRunResult {
    pub(crate) outcome: AttemptOutcome,
    pub(crate) command_rx: mpsc::UnboundedReceiver<SyncCommand>,
}

pub(crate) struct AttemptContextParams {
    pub(crate) app: AppHandle,
    pub(crate) session_id: u64,
    pub(crate) attempt_id: u64,
    pub(crate) cancel: CancellationToken,
    pub(crate) tx: mpsc::UnboundedSender<SyncCommand>,
    pub(crate) status: Arc<RwLock<String>>,
    pub(crate) command_rx: mpsc::UnboundedReceiver<SyncCommand>,
    pub(crate) pipeline_rx: mpsc::UnboundedReceiver<PipelineCommand>,
    pub(crate) ws: SyncWebSocket,
    pub(crate) http: Client,
    pub(crate) http_url: String,
    pub(crate) token: String,
    pub(crate) prerender_enabled: bool,
    pub(crate) write_queue: Arc<DbWriteQueue>,
    pub(crate) logger: Arc<Mutex<SyncLogger>>,
    pub(crate) semaphore: Arc<NetworkAwareSemaphore>,
    pub(crate) pipeline: Arc<SyncPipeline>,
}

pub(crate) struct AttemptContext {
    pub(crate) app: AppHandle,
    pub(crate) session_id: u64,
    pub(crate) attempt_id: u64,
    pub(crate) cancel: CancellationToken,
    pub(crate) tx: mpsc::UnboundedSender<SyncCommand>,
    pub(crate) status: Arc<RwLock<String>>,
    pub(crate) command_rx: mpsc::UnboundedReceiver<SyncCommand>,
    pub(crate) pipeline_rx: mpsc::UnboundedReceiver<PipelineCommand>,
    pub(crate) ws: SyncWebSocket,
    pub(crate) http: Client,
    pub(crate) http_url: String,
    pub(crate) token: String,
    pub(crate) prerender_enabled: bool,
    pub(crate) write_queue: Arc<DbWriteQueue>,
    pub(crate) logger: Arc<Mutex<SyncLogger>>,
    pub(crate) task_tracker: Arc<SyncTaskTracker>,
    pub(crate) pipeline: Arc<SyncPipeline>,
    pub(crate) semaphore: Arc<NetworkAwareSemaphore>,
    pub(crate) phase_gate: Arc<Mutex<HashSet<String>>>,
    pub(crate) uploaded_hashes: Arc<RwLock<HashSet<String>>>,
    pub(crate) pending_tasks: Arc<AtomicU32>,
    pub(crate) total_tasks: Arc<AtomicU32>,
    pub(crate) pending_topics: Arc<Phase3Tracker>,
    pub(crate) expected_phase3_batch: Arc<AsyncMutex<HashSet<TopicKey>>>,
    pub(crate) expected_phase3_states: Arc<AsyncMutex<Phase3MessageSnapshots>>,
    pub(crate) phase3_inflight: Arc<AtomicBool>,
    pub(crate) pending_batches: Arc<AsyncMutex<VecDeque<PendingDiffBatch>>>,
    pub(crate) changed_topics: Arc<AsyncMutex<Vec<TopicKey>>>,
    pub(crate) changed_owners: Arc<AsyncMutex<HashSet<OwnerKey>>>,
    pub(crate) expected_manifest_count: Arc<AtomicU32>,
    pub(crate) manifest_responses_received: Arc<AtomicU32>,
    pub(crate) expected_manifest_types: Arc<Mutex<HashSet<ManifestType>>>,
    pub(crate) manifest_phase: Arc<AtomicU8>,
    pub(crate) expected_topic_hash_results: Arc<AsyncMutex<Option<HashSet<TopicKey>>>>,
    pub(crate) message_phase_barrier: MessagePhaseBarrier,
    pub(crate) awaiting_final_ack: PendingFinalAck,
    pub(crate) heartbeat: Interval,
    pub(crate) success: bool,
    pub(crate) fatal: bool,
    pub(crate) retry: Option<RetryReason>,
}

impl AttemptContext {
    pub(crate) fn new(params: AttemptContextParams) -> Self {
        let task_tracker = Arc::new(SyncTaskTracker::new(params.cancel.child_token()));
        Self {
            app: params.app,
            session_id: params.session_id,
            attempt_id: params.attempt_id,
            cancel: params.cancel,
            tx: params.tx,
            status: params.status,
            command_rx: params.command_rx,
            pipeline_rx: params.pipeline_rx,
            ws: params.ws,
            http: params.http,
            http_url: params.http_url,
            token: params.token,
            prerender_enabled: params.prerender_enabled,
            write_queue: params.write_queue,
            logger: params.logger,
            task_tracker,
            pipeline: params.pipeline,
            semaphore: params.semaphore,
            phase_gate: Arc::new(Mutex::new(HashSet::new())),
            uploaded_hashes: Arc::new(RwLock::new(HashSet::new())),
            pending_tasks: Arc::new(AtomicU32::new(0)),
            total_tasks: Arc::new(AtomicU32::new(0)),
            pending_topics: Arc::new(Phase3Tracker {
                session_id: params.session_id,
                attempt_id: params.attempt_id,
                completed: tokio::sync::Mutex::new(HashSet::new()),
                modified: tokio::sync::Mutex::new(HashSet::new()),
                failed: tokio::sync::Mutex::new(HashSet::new()),
                legacy_attachment_warnings: std::sync::atomic::AtomicUsize::new(0),
                total: std::sync::atomic::AtomicUsize::new(0),
            }),
            expected_phase3_batch: Arc::new(AsyncMutex::new(HashSet::new())),
            expected_phase3_states: Arc::new(AsyncMutex::new(Phase3MessageSnapshots::new())),
            phase3_inflight: Arc::new(AtomicBool::new(false)),
            pending_batches: Arc::new(AsyncMutex::new(VecDeque::new())),
            changed_topics: Arc::new(AsyncMutex::new(Vec::new())),
            changed_owners: Arc::new(AsyncMutex::new(HashSet::new())),
            expected_manifest_count: Arc::new(AtomicU32::new(0)),
            manifest_responses_received: Arc::new(AtomicU32::new(0)),
            expected_manifest_types: Arc::new(Mutex::new(HashSet::new())),
            manifest_phase: Arc::new(AtomicU8::new(1)),
            expected_topic_hash_results: Arc::new(AsyncMutex::new(None)),
            message_phase_barrier: MessagePhaseBarrier::default(),
            awaiting_final_ack: Arc::new(Mutex::new(None)),
            heartbeat: interval(std::time::Duration::from_secs(15)),
            success: false,
            fatal: false,
            retry: None,
        }
    }

    pub(crate) async fn run(mut self) -> AttemptRunResult {
        let mut action = AttemptAction::Continue;
        while action == AttemptAction::Continue {
            action = self.next_action().await;
        }
        self.cancel.cancel();
        self.task_tracker.close_and_wait().await;
        AttemptRunResult {
            outcome: AttemptOutcome {
                success: self.success,
                fatal: self.fatal,
                retry: self.retry,
            },
            command_rx: self.command_rx,
        }
    }

    async fn next_action(&mut self) -> AttemptAction {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => AttemptAction::Stop,
            _ = self.heartbeat.tick() => self.send_heartbeat().await,
            Some(command) = self.pipeline_rx.recv() => {
                phase::handle_pipeline_command(self, command).await
            }
            Some(command) = self.command_rx.recv() => {
                commands::handle_command(self, command).await
            }
            message = self.ws.next() => frames::handle_ws_result(self, message).await,
            else => AttemptAction::Stop,
        }
    }

    async fn send_heartbeat(&mut self) -> AttemptAction {
        if self.ws.send(Message::Ping(Vec::new().into())).await.is_ok() {
            AttemptAction::Continue
        } else {
            AttemptAction::Stop
        }
    }

    pub(crate) async fn close(&mut self) {
        let _ = self.ws.close(None).await;
    }

    pub(crate) fn stale_attempt(&self, command_attempt: u64) -> bool {
        command_attempt != self.attempt_id
    }

    pub(crate) fn mark_fatal(&mut self) {
        self.fatal = true;
    }

    pub(crate) fn mark_success(&mut self) {
        self.success = true;
    }

    pub(crate) fn mark_retry(&mut self, code: &str, message: String) {
        self.retry = Some(RetryReason {
            code: code.to_string(),
            message,
        });
    }
}
