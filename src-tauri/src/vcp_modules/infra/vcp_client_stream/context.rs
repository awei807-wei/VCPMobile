use super::super::{ActiveRequestRegistry, StreamEvent};
use crate::vcp_modules::aurora_pipeline::AuroraBuffer;
use crate::vcp_modules::chat::topic_types::MessageKey;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, AppHandle, Runtime};

#[cfg(not(target_os = "android"))]
use futures_util::Stream;
#[cfg(target_os = "android")]
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

#[path = "context/aurora.rs"]
mod aurora;
#[path = "context/connection.rs"]
mod connection;
#[path = "context/reader.rs"]
mod reader;
#[path = "context/retry.rs"]
mod retry;

#[cfg(target_os = "android")]
type TcpReader = FramedRead<tokio::net::TcpStream, LengthDelimitedCodec>;

#[cfg(not(target_os = "android"))]
type LineStream = Box<dyn Stream<Item = Result<String, std::io::Error>> + Unpin + Send>;

#[cfg(target_os = "android")]
pub(super) enum StreamSource {
    Tcp(TcpReader),
}

#[cfg(not(target_os = "android"))]
pub(super) enum StreamSource {
    Lines(LineStream),
}

#[derive(Clone, Copy)]
pub(super) enum State {
    Init,
    Connecting,
    Resuming,
    Streaming,
    Aligning,
    Retrying,
}

pub(super) enum StreamControl {
    Continue(State),
    Complete(Result<(Value, bool), String>),
}

pub(super) struct StreamSession<R: Runtime> {
    pub(super) app: AppHandle<R>,
    pub(super) client: Client,
    pub(super) final_url: String,
    pub(super) api_key: String,
    pub(super) request_body: Value,
    pub(super) message_id: String,
    pub(super) request_key: MessageKey,
    pub(super) request_epoch: u64,
    pub(super) context: Option<Value>,
    pub(super) abort_rx: tokio::sync::oneshot::Receiver<()>,
    pub(super) active_requests: Arc<ActiveRequestRegistry>,
    pub(super) stream_channel: Option<Channel<StreamEvent>>,
    pub(super) is_resume: bool,
    pub(super) last_event_index: Option<i64>,
    pub(super) initial_content: Option<String>,
    pub(super) last_finish_reason: Option<String>,
    pub(super) last_received_index: Option<i64>,
    pub(super) aurora_buffer: AuroraBuffer,
    pub(super) pending_aurora_chunk: String,
    pub(super) last_aurora_parse: Instant,
    pub(super) retry_count: u32,
    pub(super) backoff: Duration,
    pub(super) source: Option<StreamSource>,
    state: State,
}

pub(super) struct StreamRequest<R: Runtime> {
    pub(super) app: AppHandle<R>,
    pub(super) client: Client,
    pub(super) final_url: String,
    pub(super) api_key: String,
    pub(super) request_body: Value,
    pub(super) message_id: String,
    pub(super) request_key: MessageKey,
    pub(super) request_epoch: u64,
    pub(super) context: Option<Value>,
    pub(super) abort_rx: tokio::sync::oneshot::Receiver<()>,
    pub(super) active_requests: Arc<ActiveRequestRegistry>,
    pub(super) stream_channel: Option<Channel<StreamEvent>>,
    pub(super) is_resume: bool,
    pub(super) last_event_index: Option<i64>,
    pub(super) initial_content: Option<String>,
}

impl<R: Runtime> StreamSession<R> {
    fn new(request: StreamRequest<R>) -> Self {
        let StreamRequest {
            app,
            client,
            final_url,
            api_key,
            request_body,
            message_id,
            request_key,
            request_epoch,
            context,
            abort_rx,
            active_requests,
            stream_channel,
            is_resume,
            last_event_index,
            initial_content,
        } = request;
        Self {
            app,
            client,
            final_url,
            api_key,
            request_body,
            message_id,
            request_key,
            request_epoch,
            context,
            abort_rx,
            active_requests,
            stream_channel,
            is_resume,
            last_event_index,
            initial_content,
            last_finish_reason: None,
            last_received_index: last_event_index,
            aurora_buffer: AuroraBuffer::new(),
            pending_aurora_chunk: String::new(),
            last_aurora_parse: Instant::now() - Duration::from_millis(33),
            retry_count: 0,
            backoff: Duration::from_millis(500),
            source: None,
            state: State::Init,
        }
    }

    async fn run(mut self) -> Result<(Value, bool), String> {
        loop {
            let control = match self.state {
                State::Init => self.init_control(),
                State::Connecting => connection::connect(&mut self).await,
                State::Resuming => connection::resume(&mut self).await,
                State::Streaming => reader::consume(&mut self).await,
                State::Aligning => retry::align(&mut self),
                State::Retrying => retry::retry(&mut self).await,
            };
            match control {
                StreamControl::Continue(next) => self.state = next,
                StreamControl::Complete(result) => return result,
            }
        }
    }

    fn init_control(&mut self) -> StreamControl {
        self.initialize_content();
        StreamControl::Continue(if self.is_resume {
            State::Resuming
        } else {
            State::Connecting
        })
    }

    pub(super) fn send_stream_event(&self, event: StreamEvent) {
        if let Some(channel) = &self.stream_channel {
            let _ = channel.send(event);
        }
    }

    pub(super) fn remove_active_request(&self) {
        self.active_requests
            .remove_if_current(&self.request_key, self.request_epoch);
    }

    pub(super) fn partial_result(&self) -> (Value, bool) {
        (
            json!({
                "fullContent": self.aurora_buffer.full_text,
                "streamingStarted": true,
                "finishReason": self.last_finish_reason
            }),
            false,
        )
    }

    #[cfg(target_os = "android")]
    pub(super) async fn stop_helper(&self) {
        let _ = super::super::transport::send_stop_to_helper(
            &self.app,
            &self.message_id,
            &self.request_key,
        )
        .await;
    }

    #[cfg(target_os = "android")]
    pub(super) async fn connect_helper(
        &self,
        command: &str,
        params: Option<Value>,
    ) -> Result<tokio::net::TcpStream, String> {
        super::super::transport::connect_to_helper(&self.app, command, &self.message_id, params)
            .await
    }
}

pub(super) async fn run_streaming_request<R: Runtime>(
    request: StreamRequest<R>,
) -> Result<(Value, bool), String> {
    StreamSession::new(request).run().await
}
