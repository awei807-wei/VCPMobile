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

pub(super) struct ProtocolSessionContext<'a> {
    pub(super) app: &'a AppHandle,
    pub(super) device_name: &'a str,
    pub(super) ws_tx: &'a WsSink,
    pub(super) status: &'a Arc<RwLock<DistributedStatus>>,
    pub(super) registry: &'a Arc<ToolRegistry>,
    pub(super) session_id: u64,
    pub(super) session_generation: &'a Arc<AtomicU64>,
    pub(super) cancel_token: &'a CancellationToken,
    pub(super) task_registry: &'a SessionTaskRegistry,
}

struct ToolRequestTask {
    app: AppHandle,
    request_id: String,
    tool_name: String,
    tool_args: Value,
    ws_tx: WsSink,
    registry: Arc<ToolRegistry>,
    session_id: u64,
    session_generation: Arc<AtomicU64>,
    cancel_token: CancellationToken,
}

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

    pub(super) async fn handle_incoming(text: &str, context: &ProtocolSessionContext<'_>) {
        if !is_session_current(
            context.session_generation,
            context.session_id,
            context.cancel_token,
        ) {
            return;
        }
        let Some(envelope) = Self::parse_incoming(text) else {
            return;
        };
        Self::dispatch_incoming(envelope, context).await;
    }

    async fn dispatch_incoming(envelope: IncomingEnvelope, context: &ProtocolSessionContext<'_>) {
        match envelope.parse() {
            IncomingMessage::ConnectionAck {
                server_id,
                client_id,
            } => {
                Self::handle_connection_ack(context, server_id, client_id).await;
            }
            IncomingMessage::ExecuteTool {
                request_id,
                tool_name,
                tool_args,
            } => {
                Self::handle_execute_tool_request(context, request_id, tool_name, tool_args).await;
            }
            IncomingMessage::Unknown(message_type) => {
                log::debug!("[Distributed] 未知消息类型：{}", message_type);
            }
        }
    }

    async fn handle_connection_ack(
        context: &ProtocolSessionContext<'_>,
        server_id: String,
        client_id: String,
    ) {
        Self::mark_connected(
            context.status,
            context.session_id,
            context.session_generation,
            server_id,
            client_id,
        )
        .await;
        if !is_session_current(
            context.session_generation,
            context.session_id,
            context.cancel_token,
        ) {
            return;
        }
        Self::register_tools(context).await;
        if is_session_current(
            context.session_generation,
            context.session_id,
            context.cancel_token,
        ) {
            Self::emit_status_for_session(
                context.app,
                context.status,
                context.session_id,
                context.session_generation,
                context.cancel_token,
            )
            .await;
        }
        Self::spawn_ip_report(
            context.device_name,
            context.ws_tx,
            context.session_id,
            context.session_generation,
            context.cancel_token,
            context.task_registry,
        )
        .await;
        Self::push_static_placeholders(context).await;
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

    async fn handle_execute_tool_request(
        context: &ProtocolSessionContext<'_>,
        request_id: String,
        tool_name: String,
        tool_args: Value,
    ) {
        log::info!(
            "[Distributed] 收到工具执行请求：{}（requestId={}）",
            tool_name,
            request_id
        );
        let task = ToolRequestTask {
            app: context.app.clone(),
            request_id,
            tool_name,
            tool_args,
            ws_tx: context.ws_tx.clone(),
            registry: context.registry.clone(),
            session_id: context.session_id,
            session_generation: context.session_generation.clone(),
            cancel_token: context.cancel_token.clone(),
        };
        let _ = context
            .task_registry
            .spawn(async move {
                Self::run_tool_request(task).await;
            })
            .await;
    }

    async fn run_tool_request(task: ToolRequestTask) {
        let ToolRequestTask {
            app,
            request_id,
            tool_name,
            tool_args,
            ws_tx,
            registry,
            session_id,
            session_generation,
            cancel_token,
        } = task;
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

    pub(super) async fn register_tools(context: &ProtocolSessionContext<'_>) {
        if context.session_generation.load(Ordering::SeqCst) != context.session_id {
            return;
        }
        let tools = match context.registry.get_all_manifests(context.app).await {
            Ok(tools) => tools,
            Err(error) => {
                log::error!("[Distributed] 读取工具 allowlist 失败，未执行远端注册: {error}");
                Self::clear_registered_tools(context.status, context.session_id).await;
                return;
            }
        };

        let count = tools.len();
        let msg = OutgoingMessage::RegisterTools {
            server_name: context.device_name.to_string(),
            tools,
        };
        if !Self::send_message(
            context.ws_tx,
            &msg,
            context.session_id,
            context.session_generation,
            context.cancel_token,
        )
        .await
        {
            Self::clear_registered_tools(context.status, context.session_id).await;
            return;
        }
        let mut current = context.status.write().await;
        if current.session_id == context.session_id
            && context.session_generation.load(Ordering::SeqCst) == context.session_id
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

    pub(super) async fn push_static_placeholders(context: &ProtocolSessionContext<'_>) {
        support::push_static_placeholders(context).await;
    }
}
