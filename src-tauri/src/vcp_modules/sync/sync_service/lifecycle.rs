use super::errors::encode_sync_command_error;
use super::session::run_sync_session;
use super::types::{SyncCommand, SyncSessionHandle, SyncState};
use crate::vcp_modules::vcp_log_service::get_vcp_log_status_internal;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub fn init_sync_service(_app_handle: AppHandle) -> SyncState {
    SyncState {
        ws_sender: Default::default(),
        connection_status: std::sync::Arc::new(tokio::sync::RwLock::new(
            "disconnected".to_string(),
        )),
        current_log_path: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        current_logger: std::sync::Arc::new(std::sync::RwLock::new(None)),
        lifecycle: tokio::sync::Mutex::new(()),
        owner_commit: tokio::sync::Mutex::new(()),
        session: tokio::sync::Mutex::new(None),
        next_session_id: std::sync::atomic::AtomicU64::new(0),
        current_session_id: std::sync::atomic::AtomicU64::new(0),
    }
}

#[tauri::command]
pub async fn stop_sync(_handle: AppHandle, state: State<'_, SyncState>) -> Result<(), String> {
    let _lifecycle = state.lifecycle.lock().await;
    invalidate_owner(&state).await;
    let session = state.session.lock().await.take();
    let join_result = match session {
        Some(session) => cancel_and_join_session(session).await,
        None => Ok(()),
    };
    *state.connection_status.write().await = "disconnected".to_string();
    clear_runtime_state(&state)?;
    *state.current_log_path.write().await = None;
    join_result.map_err(|detail| encode_sync_command_error("SYNC_STOP_FAILED", &detail))
}

async fn invalidate_owner(state: &SyncState) {
    let _owner = state.owner_commit.lock().await;
    let generation = state
        .next_session_id
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    state
        .current_session_id
        .store(generation, std::sync::atomic::Ordering::SeqCst);
    state.ws_sender.clear();
}

fn clear_runtime_state(state: &SyncState) -> Result<(), String> {
    *state.current_logger.write().map_err(|_| {
        encode_sync_command_error(
            "SYNC_STATE_POISONED",
            "Sync logger state lock is poisoned while stopping",
        )
    })? = None;
    Ok(())
}

pub(crate) async fn cancel_and_join_session(session: SyncSessionHandle) -> Result<(), String> {
    log::info!("[SyncService] Cancelling session {}", session.session_id);
    session.cancel_token.cancel();
    let _ = session.command_tx.send(SyncCommand::Cancel);
    session
        .join_handle
        .await
        .map_err(|error| format!("同步会话退出失败: {error}"))?
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, SyncState>) -> Result<String, String> {
    Ok(state.connection_status.read().await.clone())
}

#[tauri::command]
pub async fn is_sync_active(state: State<'_, SyncState>) -> Result<bool, String> {
    Ok(state
        .session
        .lock()
        .await
        .as_ref()
        .is_some_and(|session| !session.join_handle.is_finished()))
}

#[tauri::command]
pub async fn start_manual_sync(
    handle: AppHandle,
    state: State<'_, SyncState>,
) -> Result<u64, String> {
    let _lifecycle = state.lifecycle.lock().await;
    reap_finished_session(&state).await?;
    ensure_vcp_log_connected().await?;
    let (tx, rx) = create_session_command_channel()?;
    let session_id = state
        .next_session_id
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1;
    prepare_new_session(&state, session_id, &tx).await;
    let cancel = CancellationToken::new();
    let join_cancel = cancel.clone();
    let status = state.connection_status.clone();
    let session_tx = tx.clone();
    let join_handle = tokio::spawn(async move {
        run_sync_session(handle, session_id, join_cancel, session_tx, rx, status).await
    });
    *state.session.lock().await = Some(SyncSessionHandle {
        session_id,
        cancel_token: cancel,
        command_tx: tx,
        join_handle,
    });
    Ok(session_id)
}

pub(super) fn create_session_command_channel() -> Result<
    (
        mpsc::UnboundedSender<SyncCommand>,
        mpsc::UnboundedReceiver<SyncCommand>,
    ),
    String,
> {
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(SyncCommand::StartManualSync).map_err(|error| {
        encode_sync_command_error("SYNC_START_CHANNEL_FAILED", &error.to_string())
    })?;
    Ok((tx, rx))
}

async fn reap_finished_session(state: &SyncState) -> Result<(), String> {
    let finished = {
        let mut session = state.session.lock().await;
        if session
            .as_ref()
            .is_some_and(|value| !value.join_handle.is_finished())
        {
            return Err(encode_sync_command_error(
                "SYNC_ALREADY_RUNNING",
                "A sync session is already running",
            ));
        }
        session.take()
    };
    if let Some(session) = finished {
        let result = session.join_handle.await.map_err(|error| {
            encode_sync_command_error("SYNC_PREVIOUS_SESSION_EXIT_FAILED", &error.to_string())
        })?;
        result.map_err(|detail| {
            encode_sync_command_error("SYNC_PREVIOUS_SESSION_EXIT_FAILED", &detail)
        })?;
    }
    Ok(())
}

async fn ensure_vcp_log_connected() -> Result<(), String> {
    let status = get_vcp_log_status_internal().await;
    if status == "connected" {
        Ok(())
    } else {
        Err(encode_sync_command_error(
            "VCP_LOG_DISCONNECTED",
            &format!("VCPLog status is {status}"),
        ))
    }
}

async fn prepare_new_session(
    state: &SyncState,
    session_id: u64,
    tx: &mpsc::UnboundedSender<SyncCommand>,
) {
    let _owner = state.owner_commit.lock().await;
    state
        .current_session_id
        .store(session_id, std::sync::atomic::Ordering::SeqCst);
    state.ws_sender.install(session_id, tx.clone());
    *state.connection_status.write().await = "disconnected".to_string();
    *state.current_log_path.write().await = None;
}
