use super::super::super::super::{ActiveRequestRegistry, GuardedTransition};
use crate::vcp_modules::chat::topic_types::MessageKey;
use serde_json::Value;
use std::future::Future;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
#[cfg(target_os = "android")]
use tauri::Runtime;

#[cfg(target_os = "android")]
use super::super::StreamSession;

#[cfg(target_os = "android")]
pub(super) struct HelperStopContext<R: Runtime> {
    app: tauri::AppHandle<R>,
    active_requests: Arc<ActiveRequestRegistry>,
    message_id: String,
    request_key: MessageKey,
    request_epoch: u64,
    stop_generation: Arc<AtomicU64>,
}

#[cfg(target_os = "android")]
impl<R: Runtime> Clone for HelperStopContext<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            active_requests: self.active_requests.clone(),
            message_id: self.message_id.clone(),
            request_key: self.request_key.clone(),
            request_epoch: self.request_epoch,
            stop_generation: self.stop_generation.clone(),
        }
    }
}

#[cfg(target_os = "android")]
pub(super) fn helper_stop_context<R: Runtime>(session: &StreamSession<R>) -> HelperStopContext<R> {
    HelperStopContext {
        app: session.app.clone(),
        active_requests: session.active_requests.clone(),
        message_id: session.message_id.clone(),
        request_key: session.request_key.clone(),
        request_epoch: session.request_epoch,
        stop_generation: session.helper_stop_generation.clone(),
    }
}

#[cfg(target_os = "android")]
pub(in super::super) async fn stop_helper_generation<R: Runtime>(
    session: &StreamSession<R>,
    generation: u64,
) {
    stop_helper_generation_with_context(helper_stop_context(session), generation).await;
}

#[cfg(target_os = "android")]
pub(super) async fn stop_helper_generation_with_context<R: Runtime>(
    context: HelperStopContext<R>,
    generation: u64,
) {
    let app = context.app;
    let message_id = context.message_id;
    let request_key = context.request_key;
    stop_helper_generation_with_transport(
        context.active_requests,
        message_id,
        request_key,
        context.request_epoch,
        context.stop_generation,
        generation,
        move |request| async move {
            super::super::super::super::transport::send_stop_to_helper(
                &app,
                &request.message_id,
                &request.request_key,
                request.generation,
            )
            .await
        },
    )
    .await;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperStopRequest {
    pub(super) message_id: String,
    pub(super) request_key: MessageKey,
    pub(super) generation: u64,
}

/// 在完整请求身份和 helper generation 的 registry 授权边界内执行 stop。
/// 生产路径传入 `send_stop_to_helper`；测试传入记录型传输实现。
pub async fn stop_helper_generation_with_transport<Stop, StopFut>(
    active_requests: Arc<ActiveRequestRegistry>,
    message_id: String,
    request_key: MessageKey,
    request_epoch: u64,
    stop_generation: Arc<AtomicU64>,
    generation: u64,
    send_stop: Stop,
) where
    Stop: FnOnce(HelperStopRequest) -> StopFut,
    StopFut: Future<Output = Result<(), String>>,
{
    if !claim_generation_stop(&stop_generation, generation, &message_id) {
        return;
    }
    let authorization_key = request_key.clone();
    let request = HelperStopRequest {
        message_id,
        request_key,
        generation,
    };
    let result = active_requests
        .with_helper_stop_authorization(
            &authorization_key,
            request_epoch,
            generation,
            || async move { send_stop(request).await },
        )
        .await;
    match result {
        Ok(GuardedTransition::Applied(())) => {}
        Ok(GuardedTransition::Skipped) => {
            log::info!(
                "[VCPClient] helper generation 清理遇到接管，仅保留新请求: messageId={}, generation={}",
                authorization_key.msg_id,
                generation
            );
        }
        Err(error) => {
            log::error!(
                "[VCPClient] helper generation 绑定失败后的精确 stop 失败: messageId={}, generation={}, error={error}",
                authorization_key.msg_id,
                generation
            );
        }
    }
}

fn claim_generation_stop(stop_generation: &AtomicU64, generation: u64, message_id: &str) -> bool {
    loop {
        let previous = stop_generation.load(Ordering::Acquire);
        if previous == generation {
            log::info!(
                "[VCPClient] helper generation stop 已请求，忽略重复清理: messageId={}, generation={}",
                message_id,
                generation
            );
            return false;
        }
        if stop_generation
            .compare_exchange(previous, generation, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}

#[cfg(target_os = "android")]
pub(super) fn verify_generation_ack<R: Runtime>(
    session: &StreamSession<R>,
    response: &Value,
) -> Result<(), String> {
    verify_generation_ack_for_key(&session.request_key, response)
}

/// 校验 helper ACK 的完整复合身份。
pub fn verify_generation_ack_for_key(
    request_key: &MessageKey,
    response: &Value,
) -> Result<(), String> {
    for (field, expected) in [
        ("requestId", request_key.msg_id.as_str()),
        ("messageId", request_key.msg_id.as_str()),
        ("ownerType", request_key.topic.owner_type.as_str()),
        ("ownerId", request_key.topic.owner_id.as_str()),
        ("topicId", request_key.topic.topic_id.as_str()),
    ] {
        if response[field].as_str() != Some(expected) {
            return Err(format!("helper generation 握手身份字段 {field} 不一致"));
        }
    }
    if response["eventType"].as_str() != Some("started") {
        return Err("helper generation 握手类型无效".to_string());
    }
    Ok(())
}
