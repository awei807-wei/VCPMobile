use super::super::tool_registry::ToolRegistry;
use super::super::types::DistributedStatus;
use super::{
    acquire_wake_lock_helper, is_distributed_connection_stale, is_session_current,
    DistributedClient, SessionTaskRegistry, WakeLockLease,
};
use futures_util::{stream::SplitStream, StreamExt};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{self, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct SessionLoopContext<'a> {
    app: &'a AppHandle,
    device_name: &'a str,
    ws_tx: &'a super::WsSink,
    status: &'a Arc<RwLock<DistributedStatus>>,
    registry: &'a Arc<ToolRegistry>,
    session_id: u64,
    session_generation: &'a Arc<AtomicU64>,
    cancel_token: &'a CancellationToken,
    task_registry: &'a SessionTaskRegistry,
}

struct SessionRuntime {
    ws_tx: super::WsSink,
    ws_rx: SplitStream<WsStream>,
    placeholder_interval: time::Interval,
    heartbeat_interval: time::Interval,
    last_inbound_at: Instant,
}

impl SessionRuntime {
    async fn prepare(ws_stream: WsStream) -> Self {
        let (ws_tx, ws_rx) = ws_stream.split();
        let ws_tx = Arc::new(Mutex::new(ws_tx));
        let mut placeholder_interval = time::interval(Duration::from_secs(30));
        placeholder_interval.tick().await;
        let mut heartbeat_interval = time::interval(super::DISTRIBUTED_HEARTBEAT_INTERVAL);
        heartbeat_interval.tick().await;
        Self {
            ws_tx,
            ws_rx,
            placeholder_interval,
            heartbeat_interval,
            last_inbound_at: Instant::now(),
        }
    }
}

impl DistributedClient {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_session(
        app: &AppHandle,
        ws_stream: WsStream,
        device_name: &str,
        cancel_token: &CancellationToken,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        re_register_rx: &mut tokio::sync::mpsc::Receiver<()>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        task_registry: &SessionTaskRegistry,
    ) -> String {
        #[cfg(target_os = "android")]
        if is_session_current(session_generation, session_id, cancel_token) {
            if let Err(error) =
                tauri_plugin_vcp_mobile::system::start_sensor_collection(app.clone())
            {
                log::warn!("[Distributed] 启动原生传感器采集失败：{}", error);
            }
        }

        let mut runtime = SessionRuntime::prepare(ws_stream).await;
        let context = SessionLoopContext {
            app,
            device_name,
            ws_tx: &runtime.ws_tx,
            status,
            registry,
            session_id,
            session_generation,
            cancel_token,
            task_registry: &task_registry,
        };
        let exit_reason = Self::run_session_loop(
            &context,
            &mut runtime.ws_rx,
            re_register_rx,
            &mut runtime.placeholder_interval,
            &mut runtime.heartbeat_interval,
            &mut runtime.last_inbound_at,
        )
        .await;
        Self::shutdown_session(app, cancel_token, task_registry).await;
        exit_reason
    }

    async fn shutdown_session(
        _app: &AppHandle,
        cancel_token: &CancellationToken,
        task_registry: &SessionTaskRegistry,
    ) {
        cancel_token.cancel();
        task_registry.shutdown().await;

        #[cfg(target_os = "android")]
        if let Err(error) = tauri_plugin_vcp_mobile::system::stop_sensor_collection(_app.clone()) {
            log::warn!("[Distributed] 停止原生传感器采集失败：{}", error);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_session_loop(
        context: &SessionLoopContext<'_>,
        ws_rx: &mut SplitStream<WsStream>,
        re_register_rx: &mut tokio::sync::mpsc::Receiver<()>,
        placeholder_interval: &mut time::Interval,
        heartbeat_interval: &mut time::Interval,
        last_inbound_at: &mut Instant,
    ) -> String {
        #[allow(unused_assignments)]
        let mut exit_reason = "连接正常关闭".to_string();
        loop {
            tokio::select! {
                msg = ws_rx.next() => {
                    if let Some(reason) =
                        Self::handle_session_poll(context, msg, last_inbound_at).await
                    {
                        exit_reason = reason;
                        break;
                    }
                }
                opt = re_register_rx.recv() => {
                    Self::handle_reregister_poll(context, opt).await;
                }
                _ = placeholder_interval.tick() => {
                    Self::push_placeholder_poll(context).await;
                }
                _ = heartbeat_interval.tick() => {
                    if let Err(reason) = Self::heartbeat_poll(context, last_inbound_at).await {
                        exit_reason = reason;
                        break;
                    }
                }
                _ = context.cancel_token.cancelled() => {
                    log::info!("[Distributed] 收到关闭信号，正在关闭会话。");
                    exit_reason = "客户端请求关闭".to_string();
                    break;
                }
            }
        }
        exit_reason
    }

    async fn handle_session_poll(
        context: &SessionLoopContext<'_>,
        msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        last_inbound_at: &mut Instant,
    ) -> Option<String> {
        Self::handle_session_message(msg, last_inbound_at, context).await
    }

    async fn handle_reregister_poll(context: &SessionLoopContext<'_>, opt: Option<()>) {
        Self::handle_reregister(
            opt,
            context.app,
            context.device_name,
            context.ws_tx,
            context.status,
            context.registry,
            context.session_id,
            context.session_generation,
            context.cancel_token,
        )
        .await;
    }

    async fn push_placeholder_poll(context: &SessionLoopContext<'_>) {
        Self::push_static_placeholders(
            context.app,
            context.device_name,
            context.ws_tx,
            context.registry,
            context.session_id,
            context.session_generation,
            context.cancel_token,
            context.task_registry,
        )
        .await;
    }

    async fn heartbeat_poll(
        context: &SessionLoopContext<'_>,
        last_inbound_at: &Instant,
    ) -> Result<(), String> {
        Self::send_heartbeat(
            context.app,
            context.ws_tx,
            last_inbound_at,
            context.session_id,
            context.session_generation,
            context.cancel_token,
        )
        .await
    }

    async fn handle_session_message(
        msg: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
        last_inbound_at: &mut Instant,
        context: &SessionLoopContext<'_>,
    ) -> Option<String> {
        match msg {
            Some(Ok(Message::Text(text))) => {
                Self::handle_text_message(text.to_string(), last_inbound_at, context).await
            }
            Some(Ok(Message::Ping(data))) => {
                Self::handle_ping_message(data, last_inbound_at, context).await
            }
            Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) => {
                *last_inbound_at = Instant::now();
                None
            }
            Some(Ok(Message::Close(reason))) => {
                let detail = reason
                    .map(|value| format!("{}（代码：{}）", value.reason, value.code))
                    .unwrap_or_else(|| "未提供原因".to_string());
                log::info!("[Distributed] 服务端关闭连接：{}", detail);
                Some(format!("服务端关闭连接：{}", detail))
            }
            Some(Err(error)) => {
                log::warn!("[Distributed] WebSocket 错误：{}", error);
                Some(format!("WebSocket 错误：{}", error))
            }
            None => {
                log::info!("[Distributed] WebSocket 流已结束。");
                Some("WebSocket 流结束（服务端断开）".to_string())
            }
            _ => None,
        }
    }

    async fn handle_text_message(
        text: String,
        last_inbound_at: &mut Instant,
        context: &SessionLoopContext<'_>,
    ) -> Option<String> {
        *last_inbound_at = Instant::now();
        Self::handle_incoming(
            context.app,
            &text,
            context.device_name,
            context.ws_tx,
            context.status,
            context.registry,
            context.session_id,
            context.session_generation,
            context.cancel_token,
            context.task_registry,
        )
        .await;
        None
    }

    async fn handle_ping_message(
        data: tokio_tungstenite::tungstenite::Bytes,
        last_inbound_at: &mut Instant,
        context: &SessionLoopContext<'_>,
    ) -> Option<String> {
        *last_inbound_at = Instant::now();
        Self::send_ws_message(
            context.ws_tx,
            Message::Pong(data),
            context.session_id,
            context.session_generation,
            context.cancel_token,
        )
        .await;
        None
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_reregister(
        opt: Option<()>,
        app: &AppHandle,
        device_name: &str,
        ws_tx: &super::WsSink,
        status: &Arc<RwLock<DistributedStatus>>,
        registry: &Arc<ToolRegistry>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) {
        if opt.is_none() {
            return;
        }
        log::info!("[Distributed] 配置变化，重新注册工具。");
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
            Self::emit_status_with_app(app, status).await;
        }
    }

    async fn send_heartbeat(
        app: &AppHandle,
        ws_tx: &super::WsSink,
        last_inbound_at: &Instant,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> Result<(), String> {
        let idle_for = last_inbound_at.elapsed();
        if is_distributed_connection_stale(idle_for) {
            let reason = format!("心跳超时：{} 秒没有收到 WebSocket 帧", idle_for.as_secs());
            log::warn!("[Distributed] {}", reason);
            let _ =
                Self::close_ws_message(ws_tx, session_id, session_generation, cancel_token).await;
            return Err(reason);
        }
        let tag = format!("distributed:connection:session:{session_id}");
        if !acquire_wake_lock_helper(app, &tag, session_id, session_generation, cancel_token) {
            return Err("连接已取消".to_string());
        }
        let _wake_lock = WakeLockLease::new(app, tag, session_generation, session_id);
        let ping_result = Self::send_ws_message(
            ws_tx,
            Message::Ping(Vec::new().into()),
            session_id,
            session_generation,
            cancel_token,
        )
        .await;
        if ping_result {
            Ok(())
        } else {
            let reason = "心跳发送失败".to_string();
            log::warn!("[Distributed] {}", reason);
            Err(reason)
        }
    }
}
