use super::super::tool_registry::ToolRegistry;
use super::super::types::*;
use super::{is_session_current, DistributedClient, WsSink};
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

impl DistributedClient {
    /// 执行工具并返回结果消息，同时按需推送回调。
    /// VCPChat 参考：handleToolExecutionRequest() 第 428-649 行。
    pub(super) async fn execute_tool(
        app: &AppHandle,
        request_id: &str,
        tool_name: &str,
        tool_args: Value,
        registry: &Arc<ToolRegistry>,
    ) -> (OutgoingMessage, Option<OutgoingMessage>) {
        let manifest = match registry.get_manifest(app, tool_name).await {
            Ok(manifest) => manifest,
            Err(error) => {
                log::error!("[Distributed] 读取工具 allowlist 失败: {error}");
                None
            }
        };
        match registry.execute(tool_name, tool_args, app).await {
            Ok(result) => {
                log::info!("[Distributed] 工具 {} 执行成功。", tool_name);
                let callback =
                    Self::build_plugin_callback_forward(request_id, tool_name, &result, &manifest);
                (
                    OutgoingMessage::ToolResult {
                        request_id: request_id.to_string(),
                        status: "success".to_string(),
                        result: Some(result),
                        error: None,
                    },
                    callback,
                )
            }
            Err(e) => {
                log::warn!("[Distributed] 工具 {} 执行失败：{}", tool_name, e);
                (
                    OutgoingMessage::ToolResult {
                        request_id: request_id.to_string(),
                        status: "error".to_string(),
                        result: None,
                        error: Some(e),
                    },
                    None,
                )
            }
        }
    }

    fn build_plugin_callback_forward(
        request_id: &str,
        tool_name: &str,
        result: &Value,
        manifest: &Option<ToolManifest>,
    ) -> Option<OutgoingMessage> {
        let push = manifest.as_ref()?.web_socket_push.as_ref()?;
        if !push.enabled {
            return None;
        }

        let mut callback_data = if push.use_plugin_result_as_message {
            match result {
                Value::Object(map) => Value::Object(map.clone()),
                other => json!({ "message": other }),
            }
        } else {
            json!({ "result": result })
        };

        if let Value::Object(map) = &mut callback_data {
            map.insert(
                "pluginName".to_string(),
                Value::String(tool_name.to_string()),
            );
            map.insert("taskId".to_string(), Value::String(request_id.to_string()));
        }

        Some(OutgoingMessage::PluginCallbackForward { callback_data })
    }

    // ================================================================
    // 辅助方法
    // ================================================================

    /// 序列化消息并通过 WebSocket 发送。
    pub(super) async fn send_message(
        ws_tx: &WsSink,
        msg: &OutgoingMessage,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> bool {
        match serde_json::to_string(msg) {
            Ok(json) => {
                Self::send_ws_message(
                    ws_tx,
                    tokio_tungstenite::tungstenite::Message::Text(json.into()),
                    session_id,
                    session_generation,
                    cancel_token,
                )
                .await
            }
            Err(e) => {
                log::error!("[Distributed] 序列化消息失败：{}", e);
                false
            }
        }
    }

    pub(super) async fn send_ws_message(
        ws_tx: &WsSink,
        message: tokio_tungstenite::tungstenite::Message,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> bool {
        let mut tx = match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, ws_tx.lock()).await {
            Ok(tx) => tx,
            Err(_) => {
                log::warn!("[Distributed] 获取 WebSocket 写锁超时。");
                return false;
            }
        };
        if !is_session_current(session_generation, session_id, cancel_token) {
            return false;
        }
        match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, tx.send(message)).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                log::warn!("[Distributed] WebSocket 发送失败：{}", error);
                false
            }
            Err(_) => {
                log::warn!("[Distributed] WebSocket 写入超时。");
                false
            }
        }
    }

    pub(super) async fn close_ws_message(
        ws_tx: &WsSink,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> bool {
        let mut tx = match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, ws_tx.lock()).await {
            Ok(tx) => tx,
            Err(_) => {
                log::warn!("[Distributed] 获取 WebSocket 关闭锁超时。");
                return false;
            }
        };
        if !is_session_current(session_generation, session_id, cancel_token) {
            return false;
        }
        match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, tx.close()).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                log::warn!("[Distributed] WebSocket 关闭失败：{}", error);
                false
            }
            Err(_) => {
                log::warn!("[Distributed] WebSocket 关闭超时。");
                false
            }
        }
    }

    /// 向 Vue 前端发送状态。
    pub(super) async fn emit_status(app: &AppHandle, status: &Arc<RwLock<DistributedStatus>>) {
        let s = status.read().await.clone();
        let _ = app.emit("vcp-distributed-status", &s);
    }

    pub(super) async fn emit_status_with_app(
        app: &AppHandle,
        status: &Arc<RwLock<DistributedStatus>>,
    ) {
        Self::emit_status(app, status).await;
    }

    pub(super) async fn emit_status_for_session(
        app: &AppHandle,
        status: &Arc<RwLock<DistributedStatus>>,
        session_id: u64,
        session_generation: &Arc<AtomicU64>,
        cancel_token: &CancellationToken,
    ) -> bool {
        if !is_session_current(session_generation, session_id, cancel_token) {
            return false;
        }
        let snapshot = status.read().await.clone();
        if !is_session_current(session_generation, session_id, cancel_token) {
            return false;
        }
        app.emit("vcp-distributed-status", &snapshot).is_ok()
    }
}
