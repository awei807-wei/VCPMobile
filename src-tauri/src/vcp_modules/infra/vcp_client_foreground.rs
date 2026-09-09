use tauri::{AppHandle, Runtime};
use tauri_plugin_vcp_mobile::stream::StreamIdentity;

/// 持有一次流式前台锁，离开请求作用域时自动释放。
pub struct StreamServiceLease<R: Runtime> {
    app: AppHandle<R>,
    agent_name: String,
    identity: StreamIdentity,
    generation: u64,
}

/// 获取带完整消息身份的前台锁。获取失败时返回 `None`，表示本轮没有锁需要释放。
pub fn acquire_stream_service<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
    identity: &StreamIdentity,
    source: &str,
) -> Option<StreamServiceLease<R>> {
    match tauri_plugin_vcp_mobile::stream::start_stream_service_with_identity_inner(
        app,
        agent_name,
        Some(identity),
    ) {
        Ok(Some(generation)) => Some(StreamServiceLease {
            app: app.clone(),
            agent_name: agent_name.to_string(),
            identity: identity.clone(),
            generation,
        }),
        Ok(None) => {
            log::error!(
                "[{}] 完整身份流式前台服务未返回 generation，拒绝保留 lease",
                source
            );
            None
        }
        Err(error) => {
            log::warn!("[{}] 流式前台服务启动失败: {}", source, error);
            None
        }
    }
}

impl<R: Runtime> Drop for StreamServiceLease<R> {
    fn drop(&mut self) {
        if let Err(error) = tauri_plugin_vcp_mobile::stream::stop_stream_service_with_identity_inner(
            &self.app,
            &self.agent_name,
            Some(&self.identity),
            Some(self.generation),
        ) {
            log::warn!("流式前台服务释放失败: {}", error);
        }
    }
}
