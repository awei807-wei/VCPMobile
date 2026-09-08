use super::super::tool_registry::ToolRegistry;
use super::super::types::*;
use super::{
    acquire_wake_lock_helper, is_session_current, DistributedClient, SessionTaskRegistry,
    WakeLockLease, WsSink,
};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[path = "client_protocol_support.rs"]
mod support;

impl DistributedClient {
    fn parse_incoming(text: &str) -> Option<IncomingEnvelope> {
        match serde_json::from_str(text) {
            Ok(envelope) => Some(envelope),
            Err(error) => {
                log::warn!("[Distributed] 消息解析失败：{}", error);
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_incoming(
        app: &AppHandle,
        text: &str,
        device_name: &str,
        ws_tx: &WsSink,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        if !is_session_current(session_generation, session_id, cancel_token) {
            return;
        }
        let Some(envelope) = Self::parse_incoming(text) else {
            return;
        };
        Self::dispatch_incoming(
            app,
            envelope,
            device_name,
            ws_tx,
            status,
            registry,
            session_id,
            session_generation,
            cancel_token,
            task_registry,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_incoming(
        app: &AppHandle,
        envelope: IncomingEnvelope,
        device_name: &str,
        ws_tx: &WsSink,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        match envelope.parse() {
            IncomingMessage::ConnectionAck {
                server_id,
                client_id,
            } => {
                Self::handle_connection_ack(
                    app,
                    device_name,
                    ws_tx,
                    status,
                    registry,
                    server_id,
                    client_id,
                    session_id,
                    session_generation,
                    cancel_token,
                    task_registry,
                )
                .await;
            }
            IncomingMessage::ExecuteTool {
                request_id,
                tool_name,
                tool_args,
            } => {
                Self::handle_execute_tool_request(
                    app,
                    request_id,
                    tool_name,
                    tool_args,
                    ws_tx,
                    registry,
                    session_id,
                    session_generation,
                    cancel_token,
                    task_registry,
                )
                .await;
            }
            IncomingMessage::Unknown(message_type) => {
                log::debug!("[Distributed] 未知消息类型：{}", message_type);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_connection_ack(
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSink,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        server_id: String,
        client_id: String,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        Self::mark_connected(status, session_id, session_generation, server_id, client_id).await;
        if !is_session_current(session_generation, session_id, cancel_token) {
            return;
        }
        Self::register_tools(
            app,
            device_name,
            ws_tx,
            registry,
            status,
            session_id,
            session_generation,
            cancel_token,
        )
        .await;
        if is_session_current(session_generation, session_id, cancel_token) {
            Self::emit_status_for_session(
                app,
                status,
                session_id,
                session_generation,
                cancel_token,
            )
            .await;
        }
        Self::spawn_ip_report(
            device_name,
            ws_tx,
            session_id,
            session_generation,
            cancel_token,
            task_registry,
        )
        .await;
        Self::push_static_placeholders(
            app,
            device_name,
            ws_tx,
            registry,
            session_id,
            session_generation,
            cancel_token,
            task_registry,
        )
        .await;
    }

    async fn mark_connected(
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        server_id: String,
        client_id: String,
    ) {
        let mut current = status.write().await;
        if current.session_id == session_id
            && session_generation.load(Ordering::SeqCst) == session_id
        {
            current.state = ConnectionState::Connected;
            current.connected = true;
            current.server_id = Some(server_id);
            current.client_id = Some(client_id);
            current.last_error = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_execute_tool_request(
        app: &AppHandle,
        request_id: String,
        tool_name: String,
        tool_args: Value,
        ws_tx: &WsSink,
        registry: &Arc<ToolRegistry>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        log::info!(
            "[Distributed] 收到工具执行请求：{}（requestId={}）",
            tool_name,
            request_id
        );
        let app = app.clone();
        let ws_tx = ws_tx.clone();
        let registry = registry.clone();
        let session_generation = session_generation.clone();
        let cancel_token = cancel_token.clone();
        let _ = task_registry
            .spawn(async move {
                Self::run_tool_request(
                    app,
                    request_id,
                    tool_name,
                    tool_args,
                    ws_tx,
                    registry,
                    session_id,
                    session_generation,
                    cancel_token,
                )
                .await;
            })
            .await;
    }

    async fn run_tool_request(
        app: AppHandle,
        request_id: String,
        tool_name: String,
        tool_args: Value,
        ws_tx: WsSink,
        registry: Arc<ToolRegistry>,
        session_id: u64,
        session_generation: Arc<AtomicU64>,
        cancel_token: CancellationToken,
    ) {
        let tag = format!("distributed:tool:{}:session:{}", request_id, session_id);
        if !acquire_wake_lock_helper(&app, &tag, session_id, &session_generation, &cancel_token) {
            return;
        }
        let _wake_lock = WakeLockLease::new(&app, tag.clone(), &session_generation, session_id);
        let result = tokio::select! {
            _ = cancel_token.cancelled() => {
                return;
            }
            result = Self::execute_tool(&app, &request_id, &tool_name, tool_args, &registry) => result,
        };
        let (response, callback) = result;
        if is_session_current(&session_generation, session_id, &cancel_token) {
            Self::send_message(
                &ws_tx,
                &response,
                session_id,
                &session_generation,
                &cancel_token,
            )
            .await;
            if let Some(callback) = callback {
                Self::send_message(
                    &ws_tx,
                    &callback,
                    session_id,
                    &session_generation,
                    &cancel_token,
                )
                .await;
            }
        }
    }

    async fn spawn_ip_report(
        device_name: &str,
        ws_tx: &WsSink,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        support::spawn_ip_report(
            device_name,
            ws_tx,
            session_id,
            session_generation,
            cancel_token,
            task_registry,
        )
        .await;
    }

    pub(super) async fn register_tools(
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSink,
        registry: &Arc<ToolRegistry>,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) {
        if session_generation.load(Ordering::SeqCst) != session_id {
            return;
        }
        let tools = match registry.get_all_manifests(app).await {
            Ok(tools) => tools,
            Err(error) => {
                log::error!("[Distributed] 读取工具 allowlist 失败，未执行远端注册: {error}");
                Self::clear_registered_tools(status, session_id).await;
                return;
            }
        };

        let count = tools.len();
        let msg = OutgoingMessage::RegisterTools {
            server_name: device_name.to_string(),
            tools,
        };
        if !Self::send_message(ws_tx, &msg, session_id, session_generation, cancel_token).await {
            Self::clear_registered_tools(status, session_id).await;
            return;
        }
        let mut current = status.write().await;
        if current.session_id == session_id
            && session_generation.load(Ordering::SeqCst) == session_id
        {
            current.registered_tools = count;
        }
        log::info!("[Distributed] 已向主服务器注册 {} 个工具。", count);
    }

    async fn clear_registered_tools(status: &Arc<RwLock<DistributedStatus>>, session_id: u64) {
        let mut current = status.write().await;
        if current.session_id == session_id {
            current.registered_tools = 0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn push_static_placeholders(
        app: &AppHandle,
        device_name: &str,
        ws_tx: &WsSink,
        registry: &Arc<ToolRegistry>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        support::push_static_placeholders(
            app,
            device_name,
            ws_tx,
            registry,
            session_id,
            session_generation,
            cancel_token,
            task_registry,
        )
        .await;
    }
}
