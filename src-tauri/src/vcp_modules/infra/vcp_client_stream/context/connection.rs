#[cfg(not(target_os = "android"))]
use super::State;
use super::{StreamControl, StreamSession};
use std::time::Duration;
use tauri::Runtime;

#[cfg(target_os = "android")]
#[path = "connection/android.rs"]
mod android;
#[path = "connection/bind.rs"]
mod bind;
#[cfg(not(target_os = "android"))]
#[path = "connection/desktop.rs"]
mod desktop;
#[path = "connection/stop.rs"]
mod stop;

#[cfg(test)]
pub(super) use bind::{
    bind_generation_with_timeout, bind_generation_with_timeout_or_cancel, BindGenerationAttempt,
};
#[cfg(test)]
pub(super) use stop::{
    stop_helper_generation_with_transport, verify_generation_ack_for_key, HelperStopRequest,
};

#[cfg(test)]
#[path = "connection/tests.rs"]
mod tests;

pub(super) async fn connect<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    #[cfg(target_os = "android")]
    {
        android::connect_android(session).await
    }
    #[cfg(not(target_os = "android"))]
    {
        desktop::connect_desktop(session).await
    }
}

pub(super) async fn resume<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    while !crate::vcp_modules::infra::lifecycle_manager::is_app_in_foreground(&session.app) {
        log::info!(
            "[VCPClient] 应用在后台，暂停消息重连：{}",
            session.message_id
        );
        tokio::select! {
            _ = &mut session.abort_rx => {
                #[cfg(target_os = "android")]
                session.stop_helper().await;
                return session.cancel_silently();
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }

    #[cfg(target_os = "android")]
    {
        android::resume_android(session).await
    }
    #[cfg(not(target_os = "android"))]
    {
        log::warn!("[VCPClient] 桌面端不支持 SSE 代理重连，进入对齐");
        StreamControl::Continue(State::Aligning)
    }
}

#[cfg(target_os = "android")]
pub(super) use stop::stop_helper_generation;
