use super::super::super::tool_registry::ToolRegistry;
use super::super::super::types::*;
use super::{
    acquire_wake_lock_helper, is_session_current, DistributedClient, SessionTaskRegistry,
    WakeLockLease, WsSink,
};
use serde_json::Value;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

pub(super) async fn spawn_ip_report(
    device_name: &str,
    ws_tx: &WsSink,
    session_id: u64,
    session_generation: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
    task_registry: &SessionTaskRegistry,
) {
    let device_name = device_name.to_string();
    let ws_tx = ws_tx.clone();
    let session_generation = session_generation.clone();
    let cancel_token = cancel_token.clone();
    let _ = task_registry
        .spawn(async move {
            report_ip(
                &device_name,
                &ws_tx,
                session_id,
                &session_generation,
                &cancel_token,
            )
            .await;
        })
        .await;
}

pub(super) async fn report_ip(
    device_name: &str,
    ws_tx: &WsSink,
    session_id: u64,
    session_generation: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
) {
    let local_ips = Vec::new();
    let fetch_public_ip = async {
        match reqwest::get("https://api.ipify.org?format=json").await {
            Ok(response) => response
                .json::<Value>()
                .await
                .ok()
                .and_then(|data| data.get("ip").and_then(|ip| ip.as_str()).map(str::to_owned)),
            Err(error) => {
                log::warn!("[Distributed] 获取公网 IP 失败：{}", error);
                None
            }
        }
    };
    let public_ip = tokio::select! {
        _ = cancel_token.cancelled() => return,
        value = tokio::time::timeout(Duration::from_secs(5), fetch_public_ip) => {
            match value {
                Ok(value) => value,
                Err(_) => {
                    log::warn!("[Distributed] 获取公网 IP 超时。");
                    None
                }
            }
        }
    };
    if !is_session_current(session_generation, session_id, cancel_token) {
        return;
    }
    let msg = OutgoingMessage::ReportIp {
        server_name: device_name.to_string(),
        local_ips,
        public_ip,
    };
    DistributedClient::send_message(ws_tx, &msg, session_id, session_generation, cancel_token)
        .await;
    log::info!("[Distributed] 已发送 IP 报告。");
}

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
    let app = app.clone();
    let device_name = device_name.to_string();
    let ws_tx = ws_tx.clone();
    let registry = registry.clone();
    let session_generation = session_generation.clone();
    let cancel_token = cancel_token.clone();
    let _ = task_registry
        .spawn(async move {
            let tag = format!("distributed:placeholder_push:session:{session_id}");
            if !acquire_wake_lock_helper(&app, &tag, session_id, &session_generation, &cancel_token)
            {
                return;
            }
            let _wake_lock = WakeLockLease::new(&app, tag, &session_generation, session_id);
            match registry.get_all_placeholder_values(&app).await {
                Ok(placeholders)
                    if !placeholders.is_empty()
                        && is_session_current(&session_generation, session_id, &cancel_token) =>
                {
                    let msg = OutgoingMessage::UpdateStaticPlaceholders {
                        server_name: device_name,
                        placeholders,
                    };
                    DistributedClient::send_message(
                        &ws_tx,
                        &msg,
                        session_id,
                        &session_generation,
                        &cancel_token,
                    )
                    .await;
                }
                Ok(_) => {}
                Err(error) => {
                    log::error!("[Distributed] 读取工具 allowlist 失败，跳过占位符推送: {error}");
                }
            }
        })
        .await;
}
