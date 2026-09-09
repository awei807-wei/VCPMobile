// distributed/client.rs
// WebSocket client for VCP Distributed Node
// Mirrors VCPChat/VCPDistributedServer/VCPDistributedServer.js (class DistributedServer)
// Self-contained — does NOT import anything from vcp_modules/.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::tool_registry::ToolRegistry;
use super::types::*;

/// Type alias for the WebSocket sink to avoid excessive complexity in signatures.
type WsSink = Arc<
    Mutex<
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            WsMessage,
        >,
    >,
>;

const DISTRIBUTED_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const DISTRIBUTED_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(75);

/// 单次连接生命周期使用的不可变配置。
pub(crate) struct ConnectionConfig {
    ws_url: String,
    vcp_key: String,
    device_name: String,
}

impl ConnectionConfig {
    pub(crate) fn new(ws_url: String, vcp_key: String, device_name: String) -> Self {
        Self {
            ws_url,
            vcp_key,
            device_name,
        }
    }
}

/// 一次经过串行化边界处理的生命周期调和请求。
pub(crate) struct ReconcileRequest {
    request_id: u64,
    enabled: bool,
    force_reconnect: bool,
    trigger_reconnect: bool,
    config: ConnectionConfig,
    registry: Arc<ToolRegistry>,
}

impl ReconcileRequest {
    pub(crate) fn new(
        request_id: u64,
        enabled: bool,
        force_reconnect: bool,
        trigger_reconnect: bool,
        config: ConnectionConfig,
        registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            request_id,
            enabled,
            force_reconnect,
            trigger_reconnect,
            config,
            registry,
        }
    }
}

/// Runtime context for a single connection lifecycle (channel receivers).
struct SessionContext {
    status: Arc<RwLock<DistributedStatus>>,
    registry: Arc<ToolRegistry>,
    re_register_rx: tokio::sync::mpsc::Receiver<()>,
    reconnect_rx: tokio::sync::mpsc::Receiver<()>,
    session_id: u64,
    session_generation: Arc<AtomicU64>,
    task_registry: SessionTaskRegistry,
}

/// Handle to an active connection session — created by start(), dropped by stop().
struct ConnectionSession {
    session_id: u64,
    cancel_token: CancellationToken,
    re_register_tx: tokio::sync::mpsc::Sender<()>,
    reconnect_tx: tokio::sync::mpsc::Sender<()>,
    task_registry: SessionTaskRegistry,
    task_handle: tokio::task::JoinHandle<()>,
}

/// 表示一个已经声明 stop 的生命周期请求。
///
/// 请求对象存活期间会阻止新的 start 越过正在等待的 stop；释放后，新的
/// start 请求可以正常建立下一代 session。
pub struct StopRequest {
    request_id: u64,
    stop_in_flight: Arc<AtomicUsize>,
    stop_barrier_epoch: Arc<AtomicU64>,
    reservation_lock: Arc<std::sync::Mutex<()>>,
}

impl StopRequest {
    pub fn request_id(&self) -> u64 {
        self.request_id
    }
}

impl Drop for StopRequest {
    fn drop(&mut self) {
        let _reservation = self
            .reservation_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.stop_in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.stop_barrier_epoch.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn build_distributed_connection_url(raw_url: &str, key: &str) -> Result<String, String> {
    let mut url = Url::parse(raw_url.trim_end_matches('/'))
        .map_err(|e| format!("Invalid distributed WebSocket URL: {}", e))?;
    match url.scheme() {
        "ws" | "wss" => {}
        "http" => url
            .set_scheme("ws")
            .map_err(|_| "Invalid distributed URL scheme: http".to_string())?,
        "https" => url
            .set_scheme("wss")
            .map_err(|_| "Invalid distributed URL scheme: https".to_string())?,
        scheme => {
            return Err(format!(
                "Unsupported distributed URL scheme: {}. Use ws:// or wss://",
                scheme
            ))
        }
    }

    let raw_path = url.path().trim_end_matches('/');
    let prefix = raw_path
        .find("/vcp-distributed-server")
        .or_else(|| raw_path.find("/VCPlog"))
        .map(|idx| &raw_path[..idx])
        .unwrap_or(raw_path)
        .trim_end_matches('/');
    let connection_path = if prefix.is_empty() || prefix == "/" {
        format!("/vcp-distributed-server/VCP_Key={}", key)
    } else {
        format!("{}/vcp-distributed-server/VCP_Key={}", prefix, key)
    };

    url.set_path(&connection_path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn redact_distributed_connection_url(connection_url: &str) -> String {
    match Url::parse(connection_url) {
        Ok(mut url) => {
            let raw_path = url.path();
            let redacted_path = raw_path
                .find("/VCP_Key=")
                .map(|idx| format!("{}/VCP_Key=***", &raw_path[..idx]))
                .unwrap_or_else(|| raw_path.to_string());
            url.set_path(&redacted_path);
            url.to_string()
        }
        Err(_) => connection_url.to_string(),
    }
}

fn is_distributed_connection_stale(idle_for: Duration) -> bool {
    idle_for >= DISTRIBUTED_HEARTBEAT_TIMEOUT
}

/// Distributed node state, shared across async tasks.
pub struct DistributedClient {
    /// Current connection status.
    status: Arc<RwLock<DistributedStatus>>,
    /// Active session handle — None when disconnected.
    /// 用标准互斥锁保护会话句柄，使 stop 声明可以同步取消当前会话。
    session: std::sync::Mutex<Option<ConnectionSession>>,
    /// 串行化 start、stop、reconcile 和网络恢复的所有状态转换。
    transition: Mutex<()>,
    /// 所有调用入口共用的请求序号，较新的请求覆盖较旧请求。
    request_epoch: AtomicU64,
    /// 将请求预留与 stop 屏障更新线性化，避免预留窗口穿过正在声明的 stop。
    reservation_lock: Arc<std::sync::Mutex<()>>,
    /// 最近一次 stop 请求的序号，用于拒绝已经过期的 start。
    stop_epoch: AtomicU64,
    /// stop 已经开始但尚未完成的数量，阻止 start 在 stop 等待锁时复活。
    stop_in_flight: Arc<AtomicUsize>,
    /// 偶数表示没有 stop，奇数表示有 stop 正在生效。
    start_barrier_epoch: Arc<AtomicU64>,
    /// 最近一次 start 预留时观察到的屏障代际。
    start_barrier_snapshot: AtomicU64,
    /// 活跃连接的单调 generation；stop 也会推进它以令旧任务失效。
    session_generation: Arc<AtomicU64>,
}

impl DistributedClient {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(DistributedStatus::default())),
            session: std::sync::Mutex::new(None),
            transition: Mutex::new(()),
            request_epoch: AtomicU64::new(0),
            reservation_lock: Arc::new(std::sync::Mutex::new(())),
            stop_epoch: AtomicU64::new(0),
            stop_in_flight: Arc::new(AtomicUsize::new(0)),
            start_barrier_epoch: Arc::new(AtomicU64::new(0)),
            start_barrier_snapshot: AtomicU64::new(0),
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 为一个希望启动或调和的调用预留序号。
    pub fn reserve_start_request(&self) -> u64 {
        let _reservation = self
            .reservation_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request_id = self.request_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.start_barrier_snapshot.store(
            self.start_barrier_epoch.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        request_id
    }

    /// 为 stop/禁用调用预留序号，并立即阻断尚未安装的旧 start。
    pub fn reserve_stop_request(&self) -> StopRequest {
        let _reservation = self
            .reservation_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request_id = self.request_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.start_barrier_snapshot.store(
            self.start_barrier_epoch.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if self.stop_in_flight.fetch_add(1, Ordering::SeqCst) == 0 {
            self.start_barrier_epoch.fetch_add(1, Ordering::SeqCst);
        }
        self.stop_epoch.fetch_max(request_id, Ordering::SeqCst);
        self.invalidate_active_session();
        StopRequest {
            request_id,
            stop_in_flight: self.stop_in_flight.clone(),
            stop_barrier_epoch: self.start_barrier_epoch.clone(),
            reservation_lock: self.reservation_lock.clone(),
        }
    }

    fn request_is_latest(&self, request_id: u64) -> bool {
        self.request_epoch.load(Ordering::SeqCst) == request_id
    }

    fn start_request_is_current(&self, request_id: u64) -> bool {
        let _reservation = self
            .reservation_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.start_request_is_current_locked(request_id)
    }

    fn start_request_is_current_locked(&self, request_id: u64) -> bool {
        self.request_is_latest(request_id)
            && self.stop_epoch.load(Ordering::SeqCst) < request_id
            && self.stop_in_flight.load(Ordering::SeqCst) == 0
            && self.start_barrier_snapshot.load(Ordering::SeqCst)
                == self.start_barrier_epoch.load(Ordering::SeqCst)
    }

    fn next_session_generation(&self) -> u64 {
        self.session_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn invalidate_active_session(&self) {
        self.session_generation.fetch_add(1, Ordering::SeqCst);
        let session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(session) = session.as_ref() {
            session.cancel_token.cancel();
            session.task_registry.invalidate();
        }
    }

    fn lock_session(&self) -> std::sync::MutexGuard<'_, Option<ConnectionSession>> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[path = "client_connection.rs"]
mod connection;
#[path = "client_lifecycle.rs"]
mod lifecycle;
#[path = "client_lifecycle_start.rs"]
mod lifecycle_start;
#[path = "client_protocol.rs"]
mod protocol;
#[path = "client_session.rs"]
mod session;
#[path = "client_tasks.rs"]
mod task_registry;
#[path = "client_tools.rs"]
mod tools;
use self::task_registry::{SessionTaskRegistry, WakeLockLease, SESSION_STOP_TIMEOUT};
#[cfg(target_os = "android")]
fn acquire_wake_lock_helper(
    app: &tauri::AppHandle,
    tag: &str,
    session_id: u64,
    session_generation: &AtomicU64,
    cancel_token: &CancellationToken,
) -> bool {
    if !is_session_current(session_generation, session_id, cancel_token) {
        return false;
    }
    let scoped_tag = scoped_wake_lock_tag(tag, session_id);
    if let Err(e) = tauri_plugin_vcp_mobile::stream::acquire_foreground_inner(
        app,
        &scoped_tag,
        10, // priority = PRIORITY_DISTRIBUTED
        "[分布式连接]",
        false, // screen_keep_on = false
    ) {
        log::warn!(
            "[Distributed] 获取原生保活锁失败（标签 {}）：{}",
            scoped_tag,
            e
        );
        return false;
    }
    true
}

#[cfg(target_os = "android")]
fn release_wake_lock_helper(app: &tauri::AppHandle, tag: &str) {
    if let Err(e) = tauri_plugin_vcp_mobile::stream::release_foreground_inner(app, tag) {
        log::warn!("[Distributed] 释放原生保活锁失败（标签 {}）：{}", tag, e);
    }
}

#[cfg(not(target_os = "android"))]
fn acquire_wake_lock_helper(
    _app: &tauri::AppHandle,
    _tag: &str,
    _session_id: u64,
    _session_generation: &AtomicU64,
    _cancel_token: &CancellationToken,
) -> bool {
    is_session_current(_session_generation, _session_id, _cancel_token)
}

#[cfg(not(target_os = "android"))]
fn release_wake_lock_helper(_app: &tauri::AppHandle, _tag: &str) {}

fn is_session_current(
    session_generation: &AtomicU64,
    session_id: u64,
    cancel_token: &CancellationToken,
) -> bool {
    !cancel_token.is_cancelled() && session_generation.load(Ordering::SeqCst) == session_id
}

fn scoped_wake_lock_tag(tag: &str, session_id: u64) -> String {
    format!("{tag}:generation:{session_id}")
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
