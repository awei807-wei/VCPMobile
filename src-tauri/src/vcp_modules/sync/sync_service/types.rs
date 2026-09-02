use crate::vcp_modules::sync_executor::batch_diff_handler::Phase3ProtocolError;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::Value;
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::{tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

pub(crate) type RoutedSyncCommand = (u64, mpsc::UnboundedSender<SyncCommand>);
pub(crate) type SyncWebSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub(crate) type PendingFinalAck = Arc<Mutex<Option<FinalAckKey>>>;
pub(crate) type PendingDiffBatch = serde_json::Map<String, Value>;

#[derive(Clone, Default)]
pub struct SyncCommandRouter {
    pub(crate) current: Arc<std::sync::RwLock<Option<RoutedSyncCommand>>>,
}

impl SyncCommandRouter {
    pub fn send(&self, command: SyncCommand) -> Result<(), String> {
        let current = self
            .current
            .read()
            .map_err(|_| "同步命令路由锁已损坏".to_string())?;
        let Some((_, sender)) = current.as_ref() else {
            return Err("同步会话未运行".to_string());
        };
        sender.send(command).map_err(|e| e.to_string())
    }

    pub(crate) fn install(&self, session_id: u64, sender: mpsc::UnboundedSender<SyncCommand>) {
        if let Ok(mut current) = self.current.write() {
            *current = Some((session_id, sender));
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut current) = self.current.write() {
            *current = None;
        }
    }

    pub(crate) fn clear_if_owner(&self, session_id: u64) {
        if let Ok(mut current) = self.current.write() {
            if current.as_ref().map(|(id, _)| *id) == Some(session_id) {
                *current = None;
            }
        }
    }
}

pub struct SyncState {
    pub ws_sender: SyncCommandRouter,
    pub connection_status: Arc<RwLock<String>>,
    pub current_log_path: Arc<RwLock<Option<String>>>,
    pub current_logger: Arc<std::sync::RwLock<Option<Arc<std::sync::Mutex<SyncLogger>>>>>,
    pub(crate) lifecycle: AsyncMutex<()>,
    pub(crate) owner_commit: AsyncMutex<()>,
    pub(crate) session: AsyncMutex<Option<SyncSessionHandle>>,
    pub(crate) next_session_id: AtomicU64,
    pub(crate) current_session_id: AtomicU64,
}

pub(crate) struct SyncSessionHandle {
    pub(crate) session_id: u64,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) command_tx: mpsc::UnboundedSender<SyncCommand>,
    pub(crate) join_handle: JoinHandle<Result<(), String>>,
}

pub(crate) struct SyncTaskTracker {
    pub(crate) cancel_token: CancellationToken,
    pub(crate) closed: AtomicBool,
    pub(crate) tasks: AsyncMutex<JoinSet<()>>,
}

impl SyncTaskTracker {
    pub(crate) fn new(cancel_token: CancellationToken) -> Self {
        Self {
            cancel_token,
            closed: AtomicBool::new(false),
            tasks: AsyncMutex::new(JoinSet::new()),
        }
    }

    pub(crate) async fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let cancel_token = self.cancel_token.clone();
        let mut tasks = self.tasks.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        tasks.spawn(async move {
            tokio::select! {
                biased;
                _ = cancel_token.cancelled() => {}
                _ = future => {}
            }
        });
    }

    pub(crate) async fn close_and_wait(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::warn!("[SyncService] Session child task failed: {}", error);
            }
        }
    }
}

/// 追踪 Phase 3 中已处理完成的 topic，避免双重递减下溢。
pub struct Phase3Tracker {
    pub session_id: u64,
    pub attempt_id: u64,
    pub completed: tokio::sync::Mutex<HashSet<String>>,
    pub modified: tokio::sync::Mutex<HashSet<String>>,
    pub failed: tokio::sync::Mutex<HashSet<String>>,
    pub legacy_attachment_warnings: std::sync::atomic::AtomicUsize,
    pub total: std::sync::atomic::AtomicUsize,
}

impl Phase3Tracker {
    pub async fn mark_modified(&self, topic_id: &str) {
        self.modified.lock().await.insert(topic_id.to_string());
    }

    pub async fn mark_failed(&self, topic_id: &str) {
        self.failed.lock().await.insert(topic_id.to_string());
    }

    pub fn add_legacy_attachment_warnings(&self, count: usize) {
        self.legacy_attachment_warnings
            .fetch_add(count, Ordering::SeqCst);
    }

    pub(crate) async fn completion_summary(&self) -> SyncCompletionSummary {
        let successful_topics = self.completed.lock().await.len();
        let failed = self.failed.lock().await;
        let mut failed_topic_ids = failed.iter().cloned().collect::<Vec<_>>();
        failed_topic_ids.sort();
        failed_topic_ids.truncate(8);
        SyncCompletionSummary {
            successful_topics,
            total_topics: self.total.load(Ordering::SeqCst),
            failed_topics: failed.len(),
            legacy_attachment_warnings: self.legacy_attachment_warnings.load(Ordering::SeqCst),
            failed_topic_ids,
        }
    }

    pub async fn mark_completed(
        &self,
        topic_id: &str,
        logger: &Arc<Mutex<SyncLogger>>,
        tx: &mpsc::UnboundedSender<SyncCommand>,
        app_handle: &AppHandle,
        quiet: bool,
    ) -> bool {
        let mut completed = self.completed.lock().await;
        if !completed.insert(topic_id.to_string()) {
            return false;
        }
        let done = completed.len();
        let total = self.total.load(Ordering::SeqCst);
        if !quiet {
            if let Ok(mut logger) = logger.lock() {
                logger.log_operation("messages", "topic", topic_id, true, None);
            }
        }
        let _ = app_handle.emit(
            "vcp-sync-progress",
            serde_json::json!({
                "sessionId": self.session_id,
                "phase": "messages",
                "total": total,
                "completed": done,
                "message": format!("Syncing Messages: {}/{}", done, total),
                "successfulTopics": done,
                "totalTopics": total,
                "failedTopics": self.failed.lock().await.len(),
                "legacyAttachmentWarnings": self.legacy_attachment_warnings.load(Ordering::SeqCst)
            }),
        );
        if done == total {
            if let Ok(mut logger) = logger.lock() {
                logger.complete_phase("messages");
            }
            let _ = tx.send(SyncCommand::Finalize {
                attempt_id: self.attempt_id,
            });
        }
        true
    }
}

pub struct NetworkAwareSemaphore {
    semaphore: Arc<Semaphore>,
}

impl NetworkAwareSemaphore {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);
        let concurrency = ((cores as f32) * 1.5).clamp(6.0, 12.0) as usize;
        log::info!(
            "[Sync] Auto-optimized concurrency set to {} (cores: {})",
            concurrency,
            cores
        );
        Self {
            semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }

    pub(crate) async fn acquire(&self) -> Result<tokio::sync::SemaphorePermit<'_>, String> {
        self.semaphore
            .acquire()
            .await
            .map_err(|_| "sync concurrency gate is closed".to_string())
    }
}

pub enum SyncCommand {
    NotifyLocalChange {
        id: String,
        data_type: SyncDataType,
        hash: String,
        ts: i64,
    },
    StartAvatarMetadata {
        attempt_id: u64,
    },
    StartTopicMetadata {
        attempt_id: u64,
    },
    StartTopicValidation {
        attempt_id: u64,
    },
    StartMessages {
        attempt_id: u64,
    },
    Finalize {
        attempt_id: u64,
    },
    NotifyDelete {
        data_type: SyncDataType,
        id: String,
        deleted_at: i64,
    },
    NotifyMessageDelete {
        topic_id: String,
        message_id: String,
        deleted_at: i64,
    },
    StartManualSync,
    SendWsMessage {
        attempt_id: u64,
        value: Value,
    },
    Phase3BatchFinished {
        attempt_id: u64,
        result: Result<(), Phase3ProtocolError>,
    },
    FailAttempt {
        attempt_id: u64,
        code: &'static str,
        message: String,
    },
    FailAttemptDetailed {
        attempt_id: u64,
        code: String,
        message: String,
        failed_topic_ids: Vec<String>,
    },
    RetryAttempt {
        attempt_id: u64,
        code: &'static str,
        message: String,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalAckKey {
    pub(crate) session_id: u64,
    pub(crate) attempt_id: u64,
    pub(crate) phase: String,
    pub(crate) nonce: String,
}

impl FinalAckKey {
    pub(crate) fn new(session_id: u64, attempt_id: u64) -> Self {
        Self {
            session_id,
            attempt_id,
            phase: "messages".to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub(crate) fn message(&self) -> Value {
        serde_json::json!({
            "type": "PHASE_COMPLETED",
            "phase": self.phase,
            "sessionId": self.session_id,
            "attemptId": self.attempt_id,
            "nonce": self.nonce,
        })
    }

    pub(crate) fn matches_payload(&self, payload: &Value) -> bool {
        let Some(object) = payload.as_object() else {
            return false;
        };
        const FINAL_ACK_FIELDS: [&str; 5] = ["type", "phase", "sessionId", "attemptId", "nonce"];
        if object.len() != FINAL_ACK_FIELDS.len()
            || FINAL_ACK_FIELDS
                .iter()
                .any(|field| !object.contains_key(*field))
        {
            return false;
        }
        object.get("type").and_then(Value::as_str) == Some("PHASE_ACK")
            && object.get("phase").and_then(Value::as_str) == Some(self.phase.as_str())
            && object.get("sessionId").and_then(Value::as_u64) == Some(self.session_id)
            && object.get("attemptId").and_then(Value::as_u64) == Some(self.attempt_id)
            && object.get("nonce").and_then(Value::as_str) == Some(self.nonce.as_str())
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncCompletionSummary {
    pub(crate) successful_topics: usize,
    pub(crate) total_topics: usize,
    pub(crate) failed_topics: usize,
    pub(crate) legacy_attachment_warnings: usize,
    pub(crate) failed_topic_ids: Vec<String>,
}

/// Messages sent by the session to the desktop are kept in one place so the
/// frame dispatch module cannot accidentally introduce a Wire 1.4 message.
pub(crate) fn text_message(value: Value) -> Message {
    Message::Text(value.to_string().into())
}
