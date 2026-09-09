use super::super::tool_registry::ToolRegistry;
use super::super::types::{ConnectionState, DistributedStatus};
use super::{
    acquire_wake_lock_helper, build_distributed_connection_url, is_session_current,
    redact_distributed_connection_url, ConnectionConfig, DistributedClient, SessionContext,
    SessionTaskRegistry, WakeLockLease,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type ConnectionError = Box<tokio_tungstenite::tungstenite::Error>;

struct ConnectionLoopState {
    status: Arc<RwLock<DistributedStatus>>,
    registry: Arc<ToolRegistry>,
    re_register_rx: tokio::sync::mpsc::Receiver<()>,
    reconnect_rx: tokio::sync::mpsc::Receiver<()>,
    session_id: u64,
    session_generation: Arc<AtomicU64>,
    task_registry: SessionTaskRegistry,
    reconnect_interval: Duration,
}

impl ConnectionLoopState {
    fn new(ctx: SessionContext) -> Self {
        Self {
            status: ctx.status,
            registry: ctx.registry,
            re_register_rx: ctx.re_register_rx,
            reconnect_rx: ctx.reconnect_rx,
            session_id: ctx.session_id,
            session_generation: ctx.session_generation,
            task_registry: ctx.task_registry,
            reconnect_interval: Duration::from_secs(5),
        }
    }
}

impl DistributedClient {
    pub(super) async fn connection_loop(
        app: AppHandle,
        config: ConnectionConfig,
        cancel_token: CancellationToken,
        ctx: SessionContext,
    ) {
        let mut state = ConnectionLoopState::new(ctx);
        while Self::run_connection_iteration(&app, &config, &cancel_token, &mut state).await {}
        Self::mark_connection_loop_disconnected(
            &app,
            &state.status,
            state.session_id,
            &state.session_generation,
        )
        .await;
        log::info!("[Distributed] 连接循环已退出。");
    }

    async fn run_connection_iteration(
        app: &AppHandle,
        config: &ConnectionConfig,
        cancel_token: &CancellationToken,
        state: &mut ConnectionLoopState,
    ) -> bool {
        if !is_session_current(&state.session_generation, state.session_id, cancel_token) {
            return false;
        }
        let connection_url = match build_distributed_connection_url(&config.ws_url, &config.vcp_key)
        {
            Ok(url) => url,
            Err(error) => {
                Self::handle_invalid_url(
                    app,
                    &state.status,
                    state.session_id,
                    &state.session_generation,
                    error,
                )
                .await;
                return false;
            }
        };
        log::info!(
            "[Distributed] 正在连接主服务器：{}",
            redact_distributed_connection_url(&connection_url)
        );
        if !Self::handle_connection_attempt(app, config, &connection_url, cancel_token, state).await
        {
            return false;
        }
        if !is_session_current(&state.session_generation, state.session_id, cancel_token) {
            return false;
        }
        log::info!(
            "[Distributed] 将在 {} 秒后重连。",
            state.reconnect_interval.as_secs()
        );
        if !Self::wait_for_reconnect(
            state.reconnect_interval,
            &mut state.reconnect_rx,
            cancel_token,
        )
        .await
        {
            return false;
        }
        state.reconnect_interval =
            std::cmp::min(state.reconnect_interval * 2, Duration::from_secs(60));
        true
    }

    async fn handle_connection_attempt(
        app: &AppHandle,
        config: &ConnectionConfig,
        connection_url: &str,
        cancel_token: &CancellationToken,
        state: &mut ConnectionLoopState,
    ) -> bool {
        match Self::connect_once(
            app,
            connection_url,
            state.session_id,
            &state.session_generation,
            cancel_token,
        )
        .await
        {
            Some(Ok(ws_stream)) => {
                if !is_session_current(&state.session_generation, state.session_id, cancel_token) {
                    return false;
                }
                log::info!("[Distributed] WebSocket 连接成功。");
                state.reconnect_interval = Duration::from_secs(5);
                Self::run_connected_session(
                    app,
                    ws_stream,
                    config,
                    cancel_token,
                    &state.status,
                    &state.registry,
                    &mut state.re_register_rx,
                    state.session_id,
                    &state.session_generation,
                    &state.task_registry,
                )
                .await;
                true
            }
            Some(Err(error)) => {
                Self::record_connection_failure(
                    app,
                    &state.status,
                    state.session_id,
                    &state.session_generation,
                    error,
                )
                .await;
                true
            }
            None => false,
        }
    }

    async fn connect_once(
        app: &AppHandle,
        connection_url: &str,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> Option<Result<WsStream, ConnectionError>> {
        let tag = format!("distributed:connect:session:{session_id}");
        if !acquire_wake_lock_helper(app, &tag, session_id, session_generation, cancel_token) {
            return None;
        }
        let _wake_lock = WakeLockLease::new(app, tag, session_generation, session_id);
        let result = tokio::select! {
            result = tokio_tungstenite::connect_async(connection_url) => Some(result),
            _ = cancel_token.cancelled() => None,
        };
        match result {
            Some(Ok((stream, _response))) => Some(Ok(stream)),
            Some(Err(error)) => Some(Err(Box::new(error))),
            None => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_connected_session(
        app: &AppHandle,
        ws_stream: WsStream,
        config: &ConnectionConfig,
        cancel_token: &CancellationToken,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        re_register_rx: &mut tokio::sync::mpsc::Receiver<()>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        task_registry: &SessionTaskRegistry,
    ) {
        let exit_reason = Self::run_session(
            app,
            ws_stream,
            &config.device_name,
            cancel_token,
            status,
            registry,
            re_register_rx,
            session_id,
            session_generation,
            task_registry,
        )
        .await;
        let mut current = status.write().await;
        if current.session_id == session_id {
            if current.state != ConnectionState::Disconnecting {
                current.state = ConnectionState::Connecting;
            }
            current.connected = false;
            current.server_id = None;
            current.client_id = None;
            current.registered_tools = 0;
            current.last_error = Some(exit_reason);
        }
        drop(current);
        if session_generation.load(Ordering::SeqCst) == session_id {
            Self::emit_status(app, status).await;
        }
    }

    async fn record_connection_failure(
        app: &AppHandle,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        error: ConnectionError,
    ) {
        log::warn!("[Distributed] 连接失败：{}", error);
        let mut current = status.write().await;
        if current.session_id == session_id {
            if current.state != ConnectionState::Disconnecting {
                current.state = ConnectionState::Connecting;
            }
            current.connected = false;
            current.registered_tools = 0;
            current.last_error = Some(format!("连接失败：{}", error));
        }
        drop(current);
        if session_generation.load(Ordering::SeqCst) == session_id {
            Self::emit_status(app, status).await;
        }
    }

    async fn handle_invalid_url(
        app: &AppHandle,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        error: String,
    ) {
        log::warn!("[Distributed] 连接配置无效：{}", error);
        let mut current = status.write().await;
        if current.session_id == session_id
            && session_generation.load(Ordering::SeqCst) == session_id
        {
            current.state = ConnectionState::Disconnected;
            current.connected = false;
            current.server_id = None;
            current.client_id = None;
            current.registered_tools = 0;
            current.last_error = Some(error);
        }
        drop(current);
        if session_generation.load(Ordering::SeqCst) == session_id {
            Self::emit_status(app, status).await;
        }
    }

    async fn wait_for_reconnect(
        interval: Duration,
        reconnect_rx: &mut tokio::sync::mpsc::Receiver<()>,
        cancel_token: &CancellationToken,
    ) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(interval) => true,
            _ = reconnect_rx.recv() => {
                log::info!("[Distributed] 收到网络恢复事件，立即重连。");
                true
            }
            _ = cancel_token.cancelled() => false,
        }
    }

    async fn mark_connection_loop_disconnected(
        app: &AppHandle,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
    ) {
        let mut current = status.write().await;
        if current.session_id == session_id {
            current.state = ConnectionState::Disconnected;
            current.connected = false;
            current.server_id = None;
            current.client_id = None;
            current.registered_tools = 0;
        }
        drop(current);
        if session_generation.load(Ordering::SeqCst) == session_id {
            Self::emit_status(app, status).await;
        }
    }
}
