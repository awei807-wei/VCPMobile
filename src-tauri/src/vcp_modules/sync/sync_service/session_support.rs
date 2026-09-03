use super::connection_config::ConnectionSettings;
use super::errors::{publish_sync_error, publish_sync_nonterminal_status};
use super::logs::emit_operator_sync_log;
use super::protocol::{
    close_ws_with_deadline, parse_version_handshake_payload, schedule_sync_retry,
    send_ws_with_deadline, RetryBudget, VersionHandshakeError,
};
use super::types::SyncState;
use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use crate::vcp_modules::sync_hash::HashInitializer;
use crate::vcp_modules::sync_logger::{LogLevel, SyncLogger};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tokio_util::sync::CancellationToken;

pub(crate) async fn build_http_client(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
) -> Result<reqwest::Client, String> {
    let result = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build();
    match result {
        Ok(client) => Ok(client),
        Err(error) => {
            publish_sync_error(
                app,
                session_id,
                status,
                "HTTP_CLIENT_INIT_FAILED",
                &error.to_string(),
                Vec::new(),
            )
            .await;
            Err(error.to_string())
        }
    }
}

pub(crate) async fn create_session_resources(
    app: &AppHandle,
    session_id: u64,
) -> Result<(Arc<DbWriteQueue>, Arc<std::sync::Mutex<SyncLogger>>), String> {
    let db = app.state::<DbState>();
    let mut queue = DbWriteQueue::new(db.pool.clone(), db.path.clone());
    let settings_state = app.state::<crate::vcp_modules::settings_manager::SettingsState>();
    let configured =
        crate::vcp_modules::settings_manager::read_settings(app.clone(), settings_state)
            .await
            .ok()
            .map(|settings| settings.sync_log_level)
            .unwrap_or_else(|| "INFO".to_string());
    let level = LogLevel::parse(&configured).unwrap_or(LogLevel::Info);
    let log_dir = app
        .path()
        .app_log_dir()
        .ok()
        .map(|path| path.join("sync_logs"));
    let logger = Arc::new(std::sync::Mutex::new(SyncLogger::new_session(
        level, log_dir, session_id,
    )));
    queue.set_logger(logger.clone());
    let logger = logger;
    let path = logger
        .lock()
        .ok()
        .and_then(|guard| guard.log_path().cloned())
        .map(|path| path.to_string_lossy().to_string());
    let state = app.state::<SyncState>();
    let _owner = state.owner_commit.lock().await;
    if state.current_session_id.load(Ordering::SeqCst) == session_id {
        *state.current_log_path.write().await = path;
        *state
            .current_logger
            .write()
            .map_err(|_| "Sync logger state lock is poisoned".to_string())? = Some(logger.clone());
    }
    Ok((Arc::new(queue), logger))
}

pub(crate) async fn connect_with_cancel(
    url: &str,
    cancel: &CancellationToken,
) -> Result<super::types::SyncWebSocket, tokio_tungstenite::tungstenite::Error> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed),
        result = connect_async(url) => result.map(|(stream, _)| stream),
    }
}

pub(crate) async fn perform_handshake(
    mut ws: super::types::SyncWebSocket,
    cancel: &CancellationToken,
) -> Result<super::types::SyncWebSocket, VersionHandshakeError> {
    let version =
        crate::vcp_modules::wire_protocol::build_version_check_json(env!("CARGO_PKG_VERSION"));
    send_ws_with_deadline(&mut ws, Message::Text(version.into()))
        .await
        .map_err(VersionHandshakeError::Transport)?;
    let receive = async {
        while let Some(result) = ws.next().await {
            match result {
                Ok(Message::Text(text)) => match parse_version_handshake_payload(&text)? {
                    Some(_) => return Ok(()),
                    None => continue,
                },
                Ok(Message::Close(frame)) => return Err(close_error(frame)),
                Ok(Message::Ping(payload)) => {
                    ws.send(Message::Pong(payload))
                        .await
                        .map_err(|error| VersionHandshakeError::Transport(error.to_string()))?;
                }
                Ok(Message::Pong(_)) => {}
                Ok(Message::Binary(_)) => {
                    return Err(VersionHandshakeError::Protocol(
                        "binary handshake frame is not allowed".to_string(),
                    ))
                }
                Ok(Message::Frame(_)) => {
                    return Err(VersionHandshakeError::Protocol(
                        "raw WebSocket frame is not allowed".to_string(),
                    ))
                }
                Err(error) => return Err(VersionHandshakeError::Transport(error.to_string())),
            }
        }
        Err(VersionHandshakeError::Closed {
            code: None,
            reason: String::new(),
        })
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(VersionHandshakeError::Transport("sync cancelled".to_string())),
        result = tokio::time::timeout(super::protocol::VERSION_CHECK_TIMEOUT, receive) => result.map_err(|_| VersionHandshakeError::Transport("version handshake timed out".to_string()))?.map(|_| ws),
    }
}

pub(crate) fn close_error(
    frame: Option<tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> VersionHandshakeError {
    frame.map_or(
        VersionHandshakeError::Closed {
            code: None,
            reason: String::new(),
        },
        |frame| VersionHandshakeError::Closed {
            code: Some(frame.code.into()),
            reason: frame.reason.to_string(),
        },
    )
}

pub(crate) async fn start_owner_phase(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    ws: &mut super::types::SyncWebSocket,
) -> bool {
    let value = serde_json::json!({"type":"PHASE_START","phase":"owner_metadata"});
    if send_ws_with_deadline(ws, Message::Text(value.to_string().into()))
        .await
        .is_err()
    {
        let _ = close_ws_with_deadline(ws).await;
        emit_operator_sync_log(app, session_id, "warning", "无法启动 owner metadata 阶段");
        return false;
    }
    publish_sync_nonterminal_status(app, session_id, status, "open", "同步服务已连接").await;
    true
}

pub(crate) async fn ensure_sync_hashes(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
) -> Result<(), ()> {
    let db = app.state::<DbState>();
    if let Err(error) = HashInitializer::ensure_wire14_hashes(&db.pool).await {
        publish_sync_error(
            app,
            session_id,
            status,
            "SYNC_HASH_INIT_DB_FAILED",
            &error.to_string(),
            Vec::new(),
        )
        .await;
        return Err(());
    }
    Ok(())
}

pub(crate) async fn handle_handshake_error(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    cancel: &CancellationToken,
    retry_budget: &mut RetryBudget,
    error: VersionHandshakeError,
) -> bool {
    match error {
        VersionHandshakeError::Remote(message) => {
            publish_handshake_failure(app, session_id, status, "REMOTE_SYNC_FAILED", &message).await
        }
        VersionHandshakeError::Protocol(message) => {
            publish_handshake_failure(app, session_id, status, "VERSION_ACK_INVALID", &message)
                .await
        }
        VersionHandshakeError::Closed {
            code: Some(4001), ..
        } => {
            publish_handshake_failure(
                app,
                session_id,
                status,
                "TOKEN_MISMATCH",
                "身份认证失败（Token 错误）",
            )
            .await
        }
        VersionHandshakeError::Closed { reason, .. } => {
            schedule_handshake_retry(
                app,
                session_id,
                status,
                cancel,
                retry_budget,
                "WS_CLOSED",
                &reason,
            )
            .await
        }
        VersionHandshakeError::Transport(message) => {
            schedule_handshake_retry(
                app,
                session_id,
                status,
                cancel,
                retry_budget,
                "WS_RECEIVE_FAILED",
                &message,
            )
            .await
        }
    }
}

async fn publish_handshake_failure(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    code: &str,
    message: &str,
) -> bool {
    publish_sync_error(app, session_id, status, code, message, Vec::new()).await;
    false
}

async fn schedule_handshake_retry(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    cancel: &CancellationToken,
    retry_budget: &mut RetryBudget,
    code: &str,
    detail: &str,
) -> bool {
    schedule_sync_retry(app, session_id, status, cancel, retry_budget, code, detail).await
}

pub(crate) async fn handle_connection_error(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    cancel: &CancellationToken,
    retry_budget: &mut RetryBudget,
    settings: &ConnectionSettings,
    error: tokio_tungstenite::tungstenite::Error,
) -> bool {
    let diagnosis = super::diagnostics::diagnose_connection_failure(
        &settings.ws_url,
        &settings.http_url,
        &error,
    )
    .await;
    let fatal = matches!(
        diagnosis.error_code.as_str(),
        "CONFIG_LOOPBACK_ON_MOBILE" | "TOKEN_MISMATCH" | "WS_PATH_INVALID"
    );
    if fatal {
        publish_sync_error(
            app,
            session_id,
            status,
            &diagnosis.error_code,
            &diagnosis.error_message,
            Vec::new(),
        )
        .await;
        return false;
    }
    schedule_sync_retry(
        app,
        session_id,
        status,
        cancel,
        retry_budget,
        &diagnosis.error_code,
        &diagnosis.error_message,
    )
    .await
}

pub(crate) async fn shutdown_session(
    app: &AppHandle,
    session_id: u64,
    status: &Arc<RwLock<String>>,
    cancel: &CancellationToken,
    queue: Arc<DbWriteQueue>,
    _logger: Arc<std::sync::Mutex<SyncLogger>>,
) -> Result<(), String> {
    cancel.cancel();
    let result = queue
        .flush()
        .await
        .map_err(|error| format!("Sync session shutdown write drain failed: {error}"));
    crate::vcp_modules::sync::sync_finalize::invalidate_sync_entity_caches(app);
    let state = app.state::<SyncState>();
    let _owner = state.owner_commit.lock().await;
    if state.current_session_id.load(Ordering::SeqCst) == session_id {
        state.ws_sender.clear_if_owner(session_id);
        *state
            .current_logger
            .write()
            .map_err(|_| "Sync logger state lock is poisoned".to_string())? = None;
        *state.current_log_path.write().await = None;
        if result.is_err() {
            *status.write().await = "error".to_string();
        }
    }
    result
}
