use super::{State, StreamControl, StreamSession};
use crate::vcp_modules::infra::vcp_client::StreamEvent;
use tauri::Runtime;

pub(super) fn align<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    log::warn!("[VCPClient] Stream alignment failed (cache was empty or errored). Failing stream.");
    session.send_stream_event(StreamEvent::error(
        session.message_id.clone(),
        session.context.clone(),
        "流连接意外断开且本地缓存不可用".to_string(),
    ));
    session.remove_active_request();
    StreamControl::Complete(Ok(session.partial_result()))
}

pub(super) async fn retry<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    const MAX_RETRIES: u32 = 3;
    if session.retry_count >= MAX_RETRIES {
        return max_retries_exceeded(session, MAX_RETRIES);
    }
    session.retry_count += 1;
    session.send_stream_event(StreamEvent {
        r#type: "reconnecting".into(),
        message_id: session.message_id.clone(),
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
        "[VCPClient] Max retries reached ({}) for message: {}",
        max_retries,
        session.message_id
    );
    session.send_stream_event(StreamEvent::error(
        session.message_id.clone(),
        session.context.clone(),
        "网络连接意外断开，重连失败".to_string(),
    ));
    session.remove_active_request();
    StreamControl::Complete(Err("Max retries reached".to_string()))
}
