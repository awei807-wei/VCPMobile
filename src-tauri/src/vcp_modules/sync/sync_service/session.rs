use super::attempt::{AttemptContext, AttemptContextParams, AttemptOutcome, RetryReason};
use super::connection_config::ConnectionSettings;
use super::errors::publish_sync_nonterminal_status;
use super::logs::emit_operator_sync_log;
use super::protocol::{schedule_sync_retry, RetryBudget};
use super::session_support::{
    build_http_client, connect_with_cancel, create_session_resources, ensure_sync_hashes,
    handle_connection_error, handle_handshake_error, perform_handshake, shutdown_session,
    start_owner_phase,
};
use super::types::{NetworkAwareSemaphore, SyncCommand, SyncWebSocket};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_error::attempt_restart_code;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_pipeline::pipeline::{PipelineCommand, SyncPipeline};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

struct SessionRuntime {
    app: AppHandle,
    session_id: u64,
    cancel: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    rx: Option<mpsc::UnboundedReceiver<SyncCommand>>,
    status: Arc<RwLock<String>>,
    http: reqwest::Client,
    queue: Arc<DbWriteQueue>,
    logger: Arc<std::sync::Mutex<SyncLogger>>,
    semaphore: Arc<NetworkAwareSemaphore>,
    retry_budget: RetryBudget,
    first_attempt: bool,
    attempt_id: u64,
    successful: bool,
    fatal: bool,
    settings: ConnectionSettings,
}

pub(crate) async fn run_sync_session(
    app: AppHandle,
    session_id: u64,
    cancel: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    rx: mpsc::UnboundedReceiver<SyncCommand>,
    status: Arc<RwLock<String>>,
    settings: ConnectionSettings,
) -> Result<(), String> {
    let http = build_http_client(&app, session_id, &status).await?;
    ensure_sync_hashes(&app, session_id, &status)
        .await
        .map_err(|()| "Wire 1.4 hash initialization failed".to_string())?;
    let (queue, logger) = create_session_resources(&app, session_id).await?;
    emit_operator_sync_log(
        &app,
        session_id,
        "info",
        &format!("同步会话已锁定连接配置档：{}", settings.profile_id),
    );
    SessionRuntime {
        app,
        session_id,
        cancel,
        tx,
        rx: Some(rx),
        status,
        http,
        queue,
        logger,
        semaphore: Arc::new(NetworkAwareSemaphore::new()),
        retry_budget: RetryBudget::new(),
        first_attempt: true,
        attempt_id: 0,
        successful: false,
        fatal: false,
        settings,
    }
    .run()
    .await
}

impl SessionRuntime {
    async fn run(mut self) -> Result<(), String> {
        while self.should_continue() {
            self.run_cycle().await;
        }
        shutdown_session(
            &self.app,
            self.session_id,
            &self.status,
            &self.cancel,
            self.queue,
            self.logger,
        )
        .await
    }

    fn should_continue(&self) -> bool {
        !self.cancel.is_cancelled() && !self.successful && !self.fatal
    }

    async fn run_cycle(&mut self) {
        let Some(ws) = self.connect_and_handshake().await else {
            return;
        };
        self.run_attempt(ws).await;
    }

    async fn connect_and_handshake(&mut self) -> Option<SyncWebSocket> {
        let settings = self.settings.clone();
        publish_sync_nonterminal_status(
            &self.app,
            self.session_id,
            &self.status,
            "connecting",
            "同步服务连接中...",
        )
        .await;
        let ws = self.connect_socket(&settings).await?;
        self.handshake_socket(ws).await
    }

    async fn connect_socket(&mut self, settings: &ConnectionSettings) -> Option<SyncWebSocket> {
        let ws = match connect_with_cancel(&settings.ws_url, &self.cancel).await {
            Ok(value) => value,
            Err(error) => {
                if !handle_connection_error(
                    &self.app,
                    self.session_id,
                    &self.status,
                    &self.cancel,
                    &mut self.retry_budget,
                    settings,
                    error,
                )
                .await
                {
                    self.fatal = true;
                }
                return None;
            }
        };
        Some(ws)
    }

    async fn handshake_socket(&mut self, ws: SyncWebSocket) -> Option<SyncWebSocket> {
        match perform_handshake(ws, &self.cancel).await {
            Ok(value) => Some(value),
            Err(error) => {
                if !handle_handshake_error(
                    &self.app,
                    self.session_id,
                    &self.status,
                    &self.cancel,
                    &mut self.retry_budget,
                    error,
                )
                .await
                {
                    self.fatal = true;
                }
                None
            }
        }
    }

    async fn run_attempt(&mut self, mut ws: SyncWebSocket) {
        if !start_owner_phase(&self.app, self.session_id, &self.status, &mut ws).await {
            if !self
                .schedule_retry("WS_SEND_FAILED", "Unable to start owner metadata phase")
                .await
            {
                self.fatal = true;
            }
            return;
        }
        self.attempt_id = self.attempt_id.wrapping_add(1);
        self.publish_attempt_owner().await;
        if !self.first_attempt {
            let _ = self.tx.send(SyncCommand::StartManualSync);
        }
        self.first_attempt = false;
        let outcome = self.execute_attempt(ws).await;
        self.finish_attempt(outcome).await;
    }

    async fn publish_attempt_owner(&self) {
        let state = self.app.state::<super::types::SyncState>();
        let _owner = state.owner_commit.lock().await;
        if state.current_session_id.load(Ordering::SeqCst) == self.session_id {
            state
                .current_attempt_id
                .store(self.attempt_id, Ordering::SeqCst);
        }
    }

    async fn execute_attempt(&mut self, ws: SyncWebSocket) -> AttemptOutcome {
        let Some(command_rx) = self.rx.take() else {
            return AttemptOutcome {
                success: false,
                fatal: true,
                retry: None,
            };
        };
        let (pipeline_tx, pipeline_rx) = mpsc::unbounded_channel::<PipelineCommand>();
        let pipeline = Arc::new(SyncPipeline::new(pipeline_tx));
        let context = AttemptContext::new(AttemptContextParams {
            app: self.app.clone(),
            session_id: self.session_id,
            attempt_id: self.attempt_id,
            cancel: self.cancel.child_token(),
            tx: self.tx.clone(),
            status: self.status.clone(),
            command_rx,
            pipeline_rx,
            ws,
            http: self.http.clone(),
            http_url: self.settings.http_url.clone(),
            token: self.settings.token.clone(),
            prerender_enabled: self.settings.prerender_enabled,
            write_queue: self.queue.clone(),
            logger: self.logger.clone(),
            semaphore: self.semaphore.clone(),
            pipeline,
        });
        let result = context.run().await;
        self.rx = Some(result.command_rx);
        result.outcome
    }

    async fn finish_attempt(&mut self, outcome: AttemptOutcome) {
        self.successful = outcome.success;
        self.fatal = outcome.fatal;
        if !self.successful && !self.fatal {
            let flush_error = self.queue.flush().await.err();
            let retry = retry_reason_after_flush(outcome.retry, flush_error);
            if !self.schedule_retry(&retry.code, &retry.message).await {
                self.fatal = true;
            }
        }
    }

    async fn schedule_retry(&mut self, code: &str, detail: &str) -> bool {
        schedule_sync_retry(
            &self.app,
            self.session_id,
            &self.status,
            &self.cancel,
            &mut self.retry_budget,
            code,
            detail,
        )
        .await
    }
}

fn retry_reason_after_flush(
    attempt_retry: Option<RetryReason>,
    flush_error: Option<String>,
) -> RetryReason {
    if let Some(error) = flush_error {
        let code = attempt_restart_code("SYNC_DB_DRAIN_FAILED", &error)
            .unwrap_or_else(|| "SYNC_DB_DRAIN_FAILED".to_string());
        return RetryReason {
            code,
            message: format!("write queue drain before retry failed: {error}"),
        };
    }
    attempt_retry.unwrap_or_else(|| RetryReason {
        code: "WS_DISCONNECTED".to_string(),
        message: "同步中途异常断开".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::retry_reason_after_flush;
    use crate::vcp_modules::sync::sync_service::attempt::RetryReason;
    use crate::vcp_modules::sync_error::{encode_local_sync_error, SyncErrorStage};

    #[test]
    fn queue_flush_error_overrides_transport_retry_reason() {
        let retry = retry_reason_after_flush(
            Some(RetryReason {
                code: "WS_DISCONNECTED".to_string(),
                message: "socket closed".to_string(),
            }),
            Some("injected write failure".to_string()),
        );
        assert_eq!(retry.code, "SYNC_DB_DRAIN_FAILED");
        assert!(retry.message.contains("injected write failure"));
    }

    #[test]
    fn stale_queue_flush_error_keeps_the_restartable_snapshot_code() {
        let retry = retry_reason_after_flush(
            None,
            Some(format!(
                "rusqlite execution error: {}",
                encode_local_sync_error(
                    "SYNC_SNAPSHOT_STALE",
                    SyncErrorStage::OwnerMetadata,
                    "local Group changed",
                    Vec::new(),
                )
            )),
        );
        assert_eq!(retry.code, "SYNC_SNAPSHOT_STALE");
    }
}
