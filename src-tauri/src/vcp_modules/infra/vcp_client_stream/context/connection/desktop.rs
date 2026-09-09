use super::super::{State, StreamControl, StreamSession, StreamSource};
use futures_util::TryStreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tauri::Runtime;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;

pub(super) async fn connect_desktop<R: Runtime>(session: &mut StreamSession<R>) -> StreamControl {
    let request = session
        .client
        .post(&session.final_url)
        .header(AUTHORIZATION, format!("Bearer {}", session.api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&session.request_body)
        .send();
    let response = tokio::select! {
        _ = &mut session.abort_rx => return session.cancel_before_streaming(),
        result = request => result,
    };
    match response {
        Ok(response) if response.status().is_success() => {
            session.source = Some(StreamSource::Lines(to_line_stream(response)));
            StreamControl::Continue(State::Streaming)
        }
        Ok(response) => {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            session.fail_with_error(format!("VCP服务器错误: {} - {}", status, text))
        }
        Err(error) => {
            log::warn!("[VCPClient] 连接失败，进入重试：{:?}", error);
            StreamControl::Continue(State::Retrying)
        }
    }
}

fn to_line_stream(response: reqwest::Response) -> super::super::LineStream {
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let framed = FramedRead::new(reader, LinesCodec::new_with_max_length(512 * 1024));
    Box::new(framed.map_err(std::io::Error::other))
}
