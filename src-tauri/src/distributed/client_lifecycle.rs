use super::super::tool_registry::ToolRegistry;
use super::super::types::{ConnectionState, DistributedStatus};
use super::{DistributedClient, StopRequest, SESSION_STOP_TIMEOUT};
use std::sync::Arc;
use tauri::AppHandle;

impl DistributedClient {
    pub(super) async fn stop_existing_session(&self) {
        let old_session = { self.lock_session().take() };
        if let Some(session) = old_session {
            session.cancel_token.cancel();
            let mut task_handle = session.task_handle;
            match tokio::time::timeout(SESSION_STOP_TIMEOUT, &mut task_handle).await {
                Ok(Ok(())) => {
                    session.task_registry.shutdown().await;
                }
                Ok(Err(error)) => {
                    log::warn!("[Distributed] 连接主循环异常退出：{}", error);
                    session.task_registry.abort_and_join().await;
                }
                Err(_) => {
                    log::warn!("[Distributed] 连接主循环停止超时，执行中止。");
                    task_handle.abort();
                    let _ = task_handle.await;
                    session.task_registry.abort_and_join().await;
                }
            }
        }
    }

    /// 停止分布式节点。
    pub async fn stop(&self, app: &AppHandle) {
        let request = self.reserve_stop_request();
        self.stop_with_request(request, app).await;
    }

    /// 按已预留的请求序号停止，供设置禁用和后台 linger 复用。
    pub async fn stop_with_request(&self, request: StopRequest, app: &AppHandle) {
        self.stop_with_request_reason(request, app, None).await;
    }

    /// 停止并保留触发停止的配置错误，供权威设置读取失败时使用。
    pub async fn stop_with_request_reason(
        &self,
        request: StopRequest,
        app: &AppHandle,
        reason: Option<String>,
    ) {
        let _transition = self.transition.lock().await;
        log::info!("[Distributed] 执行停止请求序号 {}。", request.request_id);
        if let Some(reason) = reason {
            self.status.write().await.last_error = Some(reason);
        }
        self.stop_locked(app).await;
        // 在释放 transition 锁之前解除 stop barrier，避免新的启动请求越过停止。
        drop(request);
    }

    pub(super) async fn abort_start_locked(&self, app: &AppHandle, session_id: u64) {
        let mut status = self.status.write().await;
        if status.session_id == session_id && status.state == ConnectionState::Connecting {
            status.state = ConnectionState::Disconnected;
            status.connected = false;
            status.server_id = None;
            status.client_id = None;
            status.registered_tools = 0;
        }
        drop(status);
        Self::emit_status(app, &self.status).await;
    }

    pub(super) async fn stop_locked(&self, app: &AppHandle) {
        // 先推进 generation，使旧 loop、网络恢复和原生保活回调立即失效。
        let invalidated_generation = self.next_session_generation();
        {
            let mut status = self.status.write().await;
            status.session_id = invalidated_generation;
            status.state = ConnectionState::Disconnecting;
            status.connected = false;
            status.server_id = None;
            status.client_id = None;
            status.registered_tools = 0;
        }

        // 取出 session 并等待 loop 完整退出，确保 stop 返回后没有旧任务残留。
        self.stop_existing_session().await;
        {
            let mut status = self.status.write().await;
            status.state = ConnectionState::Disconnected;
            status.connected = false;
            status.server_id = None;
            status.client_id = None;
            status.registered_tools = 0;
        }
        Self::emit_status(app, &self.status).await;
    }

    /// 获取当前状态快照。
    pub async fn get_status(&self) -> DistributedStatus {
        self.status.read().await.clone()
    }

    /// 检查分布式客户端是否已连接。
    pub async fn is_connected(&self) -> bool {
        self.status.read().await.connected
    }

    /// 检查连接任务是否仍在运行（连接中、已连接或断开中）。
    pub async fn is_running(&self) -> bool {
        self.status.read().await.state != ConnectionState::Disconnected
    }

    /// 触发工具重新注册。
    pub async fn re_register_tools(&self) {
        let _transition = self.transition.lock().await;
        if let Some(session) = self.lock_session().as_ref() {
            let _ = session.re_register_tx.try_send(());
        }
    }

    /// 触发立即重连。
    pub async fn trigger_reconnect(&self) {
        let _transition = self.transition.lock().await;
        if let Some(session) = self.lock_session().as_ref() {
            let _ = session.reconnect_tx.try_send(());
        }
    }

    /// 在同一 transition 内执行设置调和或网络恢复，避免状态读取和启动/停止分离。
    pub async fn reconcile_with_request(
        &self,
        request_id: u64,
        app: &AppHandle,
        enabled: bool,
        force_reconnect: bool,
        trigger_reconnect: bool,
        ws_url: String,
        vcp_key: String,
        device_name: String,
        registry: Arc<ToolRegistry>,
    ) -> Result<(), String> {
        let _transition = self.transition.lock().await;
        if !self.start_request_is_current(request_id) {
            log::info!("[Distributed] 已忽略过期的生命周期调和请求。");
            return Ok(());
        }
        if !enabled {
            self.stop_locked(app).await;
            return Ok(());
        }

        let is_running = self.is_running().await;
        if force_reconnect && is_running {
            self.stop_locked(app).await;
        }

        if !self.start_request_is_current(request_id) {
            log::info!("[Distributed] 调和请求在停止期间已过期，跳过恢复。");
            return Ok(());
        }

        if self.is_running().await {
            if trigger_reconnect {
                if let Some(session) = self.lock_session().as_ref() {
                    let _ = session.reconnect_tx.try_send(());
                }
            }
            return Ok(());
        }

        self.start_locked(
            request_id,
            app.clone(),
            ws_url,
            vcp_key,
            device_name,
            registry,
        )
        .await
    }
}
