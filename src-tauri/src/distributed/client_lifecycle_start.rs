use super::super::tool_registry::ToolRegistry;
use super::super::types::{ConnectionState, DistributedStatus};
use super::{
    acquire_wake_lock_helper, ConnectionConfig, ConnectionSession, DistributedClient,
    SessionContext, WakeLockLease,
};
use std::sync::Arc;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

struct SessionLaunchParts {
    cancel_token: CancellationToken,
    task_registry: super::SessionTaskRegistry,
    re_register_tx: tokio::sync::mpsc::Sender<()>,
    re_register_rx: tokio::sync::mpsc::Receiver<()>,
    reconnect_tx: tokio::sync::mpsc::Sender<()>,
    reconnect_rx: tokio::sync::mpsc::Receiver<()>,
}

impl SessionLaunchParts {
    fn new() -> Self {
        let cancel_token = CancellationToken::new();
        let task_registry = super::SessionTaskRegistry::new();
        let (re_register_tx, re_register_rx) = tokio::sync::mpsc::channel(1);
        let (reconnect_tx, reconnect_rx) = tokio::sync::mpsc::channel(1);
        Self {
            cancel_token,
            task_registry,
            re_register_tx,
            re_register_rx,
            reconnect_tx,
            reconnect_rx,
        }
    }
}

impl DistributedClient {
    /// 启动分布式节点连接。
    /// `ws_url`：主服务器地址，例如 "ws://192.168.1.100:5800"。
    /// `vcp_key`：认证密钥。
    /// `device_name`：节点名称，对应 VCPChat 的 `serverName` / config.env `ServerName`。
    pub async fn start(
        &self,
        app: AppHandle,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        let request_id = self.reserve_start_request();
        self.start_with_request(request_id, app, ws_url, vcp_key, device_name, registry)
            .await
    }

    /// 按已预留的请求序号启动，供生命周期调和复用同一串行边界。
    pub async fn start_with_request(
        &self,
        request_id: u64,
        app: AppHandle,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        let _transition = self.transition.lock().await;
        self.start_locked(request_id, app, ws_url, vcp_key, device_name, registry)
            .await
    }

    pub(super) async fn start_locked(
        &self,
        request_id: u64,
        app: AppHandle,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        let next_session_id = match self.prepare_start_session(request_id).await {
            Some(session_id) => session_id,
            None => return Ok(()),
        };
        self.stop_existing_session().await;
        if !self.start_request_is_current(request_id) {
            log::info!("[Distributed] 启动准备期间收到更新请求，取消启动。");
            self.abort_start_locked(&app, next_session_id).await;
            return Ok(());
        }
        self.launch_session(
            request_id,
            app,
            next_session_id,
            ws_url,
            vcp_key,
            device_name,
            registry,
        )
        .await
    }

    async fn launch_session(
        &self,
        request_id: u64,
        app: AppHandle,
        session_id: u64,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        let parts = SessionLaunchParts::new();
        let status = self.status.clone();
        let session_generation = self.session_generation.clone();
        let Some(keepalive_lease) = self
            .prepare_keepalive_lease(
                &app,
                request_id,
                session_id,
                &parts.cancel_token,
                &session_generation,
                &status,
            )
            .await?
        else {
            return Ok(());
        };

        let (task_handle, re_register_tx, reconnect_tx) = Self::spawn_session_task(
            app.clone(),
            keepalive_lease,
            parts.cancel_token.clone(),
            parts.re_register_tx,
            parts.reconnect_tx,
            ConnectionConfig {
                ws_url,
                vcp_key,
                device_name,
            },
            Self::build_session_context(
                status,
                registry,
                parts.re_register_rx,
                parts.reconnect_rx,
                session_id,
                session_generation,
                &parts.task_registry,
            ),
        );
        self.finish_session_launch(
            &app,
            request_id,
            session_id,
            parts.cancel_token,
            re_register_tx,
            reconnect_tx,
            parts.task_registry,
            task_handle,
        )
        .await
    }

    async fn prepare_keepalive_lease(
        &self,
        app: &AppHandle,
        request_id: u64,
        session_id: u64,
        cancel_token: &CancellationToken,
        session_generation: &Arc<std::sync::atomic::AtomicU64>,
        status: &Arc<tokio::sync::RwLock<DistributedStatus>>,
    ) -> Result<Option<WakeLockLease>, String> {
        Self::emit_status(app, status).await;
        if !self.start_request_is_current(request_id) {
            self.abort_start_locked(app, session_id).await;
            return Ok(None);
        }
        if !acquire_wake_lock_helper(
            app,
            "distributed",
            session_id,
            session_generation,
            cancel_token,
        ) {
            self.abort_start_locked(app, session_id).await;
            return Err("分布式连接无法获取前台保活 lease".to_string());
        }
        Ok(Some(WakeLockLease::new(
            app,
            "distributed",
            session_generation,
            session_id,
        )))
    }

    fn spawn_session_task(
        app: AppHandle,
        keepalive_lease: WakeLockLease,
        cancel_token: CancellationToken,
        re_register_tx: tokio::sync::mpsc::Sender<()>,
        reconnect_tx: tokio::sync::mpsc::Sender<()>,
        config: ConnectionConfig,
        context: SessionContext,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Sender<()>,
        tokio::sync::mpsc::Sender<()>,
    ) {
        let task_handle = tokio::spawn(async move {
            let _keepalive_lease = keepalive_lease;
            Self::connection_loop(app, config, cancel_token, context).await;
        });
        (task_handle, re_register_tx, reconnect_tx)
    }

    async fn finish_session_launch(
        &self,
        app: &AppHandle,
        request_id: u64,
        session_id: u64,
        cancel_token: CancellationToken,
        re_register_tx: tokio::sync::mpsc::Sender<()>,
        reconnect_tx: tokio::sync::mpsc::Sender<()>,
        task_registry: super::SessionTaskRegistry,
        task_handle: tokio::task::JoinHandle<()>,
    ) -> Result<(), String> {
        let mut session = Some(ConnectionSession {
            session_id,
            cancel_token,
            re_register_tx,
            reconnect_tx,
            task_registry,
            task_handle,
        });
        let should_install = {
            let _reservation = self
                .reservation_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !self.start_request_is_current_locked(request_id) {
                false
            } else {
                *self.lock_session() = session.take();
                true
            }
        };
        if !should_install {
            let session = session
                .take()
                .expect("未安装的分布式 session 必须保留以便收口");
            session.cancel_token.cancel();
            let _ = session.task_handle.await;
            self.abort_start_locked(&app, session_id).await;
            log::info!("[Distributed] 启动安装前收到更新请求，丢弃新 session。");
        } else {
            log::info!(
                "[Distributed] 已安装 generation={} 的分布式 session。",
                session_id
            );
        }
        Ok(())
    }

    fn build_session_context(
        status: std::sync::Arc<tokio::sync::RwLock<super::super::types::DistributedStatus>>,
        registry: Arc<ToolRegistry>,
        re_register_rx: tokio::sync::mpsc::Receiver<()>,
        reconnect_rx: tokio::sync::mpsc::Receiver<()>,
        session_id: u64,
        session_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
        task_registry: &super::SessionTaskRegistry,
    ) -> super::SessionContext {
        super::SessionContext {
            status,
            registry,
            re_register_rx,
            reconnect_rx,
            session_id,
            session_generation,
            task_registry: task_registry.clone(),
        }
    }

    async fn prepare_start_session(&self, request_id: u64) -> Option<u64> {
        if !self.start_request_is_current(request_id) {
            log::info!("[Distributed] 已忽略过期的启动请求。");
            return None;
        }
        let mut status = self.status.write().await;
        if matches!(
            status.state,
            ConnectionState::Connected | ConnectionState::Connecting
        ) {
            log::info!(
                "[Distributed] 连接已处于 {:?}，跳过重复启动。",
                status.state
            );
            return None;
        }
        status.state = ConnectionState::Connecting;
        status.connected = false;
        status.server_id = None;
        status.client_id = None;
        status.last_error = None;
        status.registered_tools = 0;
        let session_id = self.next_session_generation();
        status.session_id = session_id;
        Some(session_id)
    }
}
