use super::attempt::{AttemptContext, AttemptContextParams, AttemptOutcome};
use super::errors::{publish_sync_error, publish_sync_nonterminal_status};
use super::protocol::{schedule_sync_retry, RetryBudget};
use super::session_support::{
    build_http_client, connect_with_cancel, create_session_resources, ensure_owner_hashes,
    handle_connection_error, handle_handshake_error, load_connection_settings, perform_handshake,
    shutdown_session, start_owner_phase,
};
use super::types::{NetworkAwareSemaphore, SyncCommand, SyncWebSocket};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_logger::SyncLogger;
use crate::vcp_modules::sync_pipeline::pipeline::{PipelineCommand, SyncPipeline};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

pub(crate) struct ConnectionSettings {
    pub(crate) ws_url: String,
    pub(crate) http_url: String,
    pub(crate) token: String,
    pub(crate) prerender_enabled: bool,
}

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
}

pub(crate) async fn run_sync_session(
    app: AppHandle,
    session_id: u64,
    cancel: CancellationToken,
    tx: mpsc::UnboundedSender<SyncCommand>,
    rx: mpsc::UnboundedReceiver<SyncCommand>,
    status: Arc<RwLock<String>>,
) -> Result<(), String> {
    let http = build_http_client(&app, session_id, &status).await?;
    let (queue, logger) = create_session_resources(&app, session_id).await?;
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
        let Some((ws, settings)) = self.connect_and_handshake().await else {
            return;
        };
        self.run_attempt(ws, settings).await;
    }

    async fn connect_and_handshake(&mut self) -> Option<(SyncWebSocket, ConnectionSettings)> {
        let settings = self.load_settings().await?;
        publish_sync_nonterminal_status(
            &self.app,
            self.session_id,
            &self.status,
            "connecting",
            "同步服务连接中...",
        )
        .await;
        let ws = self.connect_socket(&settings).await?;
        self.handshake_socket(ws, settings).await
    }

    async fn load_settings(&mut self) -> Option<ConnectionSettings> {
        match load_connection_settings(&self.app).await {
            Ok(value) => value,
            Err(error) => {
                publish_sync_error(
                    &self.app,
                    self.session_id,
                    &self.status,
                    "SYNC_SETTINGS_READ_FAILED",
                    &error,
                    Vec::new(),
                )
                .await;
                self.fatal = true;
                return None;
            }
        }
        .into()
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

    async fn handshake_socket(
        &mut self,
        ws: SyncWebSocket,
        settings: ConnectionSettings,
    ) -> Option<(SyncWebSocket, ConnectionSettings)> {
        match perform_handshake(ws, &self.cancel).await {
            Ok(value) => Some((value, settings)),
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

    async fn run_attempt(&mut self, mut ws: SyncWebSocket, settings: ConnectionSettings) {
        if !start_owner_phase(&self.app, self.session_id, &self.status, &mut ws).await {
            if !self
                .schedule_retry("WS_SEND_FAILED", "Unable to start owner metadata phase")
                .await
            {
                self.fatal = true;
            }
            return;
        }
        if ensure_owner_hashes(&self.app, self.session_id, &self.status)
            .await
            .is_err()
        {
            self.fatal = true;
            return;
        }
        self.attempt_id = self.attempt_id.wrapping_add(1);
        if !self.first_attempt {
            let _ = self.tx.send(SyncCommand::StartManualSync);
        }
        self.first_attempt = false;
        let outcome = self.execute_attempt(ws, settings).await;
        self.finish_attempt(outcome).await;
    }

    async fn execute_attempt(
        &mut self,
        ws: SyncWebSocket,
        settings: ConnectionSettings,
    ) -> AttemptOutcome {
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
            http_url: settings.http_url,
            token: settings.token,
            prerender_enabled: settings.prerender_enabled,
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
            let _ = self.queue.flush().await;
            crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(&self.app);
            let (code, detail) = outcome.retry.map_or_else(
                || {
                    (
                        "WS_DISCONNECTED".to_string(),
                        "同步中途异常断开".to_string(),
                    )
                },
                |retry| (retry.code, retry.message),
            );
            if !self.schedule_retry(&code, &detail).await {
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
