use super::{State, StreamControl, StreamSession};
use crate::vcp_modules::infra::vcp_client::StreamEvent;
use tauri::Runtime;

pub(super) async fn align<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    #[cfg(target_os = "android")]
    session.stop_helper().await;
    log::warn!("[VCPClient] 流对齐失败，本地缓存为空或读取出错，结束本次流");
    session.send_stream_event(StreamEvent::error(
        session.message_id.clone(),
        session.context.clone(),
        "流连接意外断开且本地缓存不可用".to_string(),
        session.request_epoch,
    ));
    session.remove_active_request();
    StreamControl::Complete(Ok(session.partial_result()))
}

pub(super) async fn retry<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    const MAX_RETRIES: u32 = 3;
    if session.retry_count >= MAX_RETRIES {
        #[cfg(target_os = "android")]
        session.stop_helper().await;
        return max_retries_exceeded(session, MAX_RETRIES);
    }
    session.retry_count += 1;
    session.send_stream_event(StreamEvent {
        r#type: "reconnecting".into(),
        message_id: session.message_id.clone(),
        generation: session.request_epoch,
        context: session.context.clone(),
        ..Default::default()
    });
    tokio::select! {
        _ = &mut session.abort_rx => {
            #[cfg(target_os = "android")]
            session.stop_helper().await;
            session.cancel_silently()
        }
        _ = tokio::time::sleep(session.backoff) => {
            session.backoff *= 2;
            StreamControl::Continue(State::Resuming)
        }
    }
}

fn max_retries_exceeded<R: Runtime>(
    session: &mut StreamSession<R>,
    max_retries: u32,
) -> StreamControl {
    log::error!(
        "[VCPClient] 已达到最大重试次数（{}），消息：{}",
        max_retries,
        session.message_id
    );
    session.send_stream_event(StreamEvent::error(
        session.message_id.clone(),
        session.context.clone(),
        "网络连接意外断开，重连失败".to_string(),
        session.request_epoch,
    ));
    session.remove_active_request();
    StreamControl::Complete(Err("达到最大重试次数".to_string()))
}
