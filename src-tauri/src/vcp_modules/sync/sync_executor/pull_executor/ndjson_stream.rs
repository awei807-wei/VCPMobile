use super::frame_validation::{
    parse_topic_ndjson_frame, validate_requested_message_ids, validate_returned_topic_identity,
    TopicNDJSONFrame,
};
use super::ndjson_codec::{parse_stream_error_frame, pull_worker_permits, NdjsonBudget};
use super::result_reporting::{
    emit_warning_logs, finish_workers, receive_results, send_empty_result, send_error_result,
    spawn_topic_worker,
};
use super::{BatchPullResult, PullProgressContext};
use crate::vcp_modules::db_write_queue::DbWriteQueue;
use futures_util::{Stream, StreamExt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_util::bytes::{Bytes, BytesMut};

struct StreamState<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    expected_messages: &'a HashMap<String, Option<HashSet<String>>>,
    expected_identities: &'a HashMap<String, (String, String)>,
    write_queue: &'a DbWriteQueue,
    prerender_enabled: bool,
    sem: Arc<Semaphore>,
    tx: mpsc::Sender<BatchPullResult>,
    workers: JoinSet<()>,
    seen_topics: HashSet<String>,
    budget: NdjsonBudget,
    buffer: BytesMut,
}

pub(crate) async fn consume_stream<R: Runtime>(
    app: &AppHandle<R>,
    response: reqwest::Response,
    expected_messages: &HashMap<String, Option<HashSet<String>>>,
    expected_identities: &HashMap<String, (String, String)>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
    progress: Option<PullProgressContext>,
) -> Result<Vec<BatchPullResult>, String> {
    let total = expected_messages.len();
    let expected_topics = expected_messages.keys().cloned().collect::<HashSet<_>>();
    let (tx, rx) = mpsc::channel::<BatchPullResult>(64);
    let receiver_handle = tokio::spawn(receive_results(app.clone(), rx, progress, total));
    let mut state = StreamState {
        app,
        expected_messages,
        expected_identities,
        write_queue,
        prerender_enabled,
        sem: Arc::new(Semaphore::new(super::PULL_WORKER_BUDGET_UNITS)),
        tx,
        workers: JoinSet::new(),
        seen_topics: HashSet::new(),
        budget: NdjsonBudget::new(total),
        buffer: BytesMut::new(),
    };
    let mut stream = response.bytes_stream();
    consume_stream_chunks(&mut stream, &mut state).await?;
    consume_trailing_line(&mut state).await?;
    ensure_expected_topics(&expected_topics, &state.seen_topics)?;
    finish_workers(state.workers, state.tx, receiver_handle).await
}

async fn consume_stream_chunks<S, R>(
    stream: &mut S,
    state: &mut StreamState<'_, R>,
) -> Result<(), String>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
    R: Runtime,
{
    while let Some(chunk_result) = tokio::time::timeout(super::NDJSON_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| "NDJSON stream idle timeout after 30 seconds".to_string())?
    {
        let chunk = chunk_result.map_err(|error| format!("Stream read error: {error}"))?;
        state.budget.observe_chunk(chunk.len())?;
        if chunk.len() > super::MAX_NDJSON_TRANSPORT_CHUNK_BYTES {
            return Err("NDJSON transport chunk exceeds 32MB budget".to_string());
        }
        state.buffer.extend_from_slice(&chunk);
        consume_complete_lines(state).await?;
        if state.buffer.len() > super::MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32MB budget".to_string());
        }
    }
    Ok(())
}

fn ensure_expected_topics(
    expected_topics: &HashSet<String>,
    seen_topics: &HashSet<String>,
) -> Result<(), String> {
    if expected_topics == seen_topics {
        return Ok(());
    }
    let mut missing = expected_topics
        .difference(seen_topics)
        .cloned()
        .collect::<Vec<_>>();
    missing.sort();
    Err(format!("NDJSON response is missing topics {missing:?}"))
}

async fn consume_trailing_line<R: Runtime>(state: &mut StreamState<'_, R>) -> Result<(), String> {
    if state.buffer.is_empty() {
        return Ok(());
    }
    let line = state.buffer.split_to(state.buffer.len()).freeze();
    consume_line(state, line).await
}

async fn consume_complete_lines<R: Runtime>(state: &mut StreamState<'_, R>) -> Result<(), String> {
    while let Some(position) = state.buffer.iter().position(|byte| *byte == b'\n') {
        let line = state.buffer.split_to(position + 1).freeze();
        consume_line(state, line).await?;
    }
    Ok(())
}

struct DecodedLine {
    frame: TopicNDJSONFrame,
    permit: OwnedSemaphorePermit,
}

async fn consume_line<R: Runtime>(
    state: &mut StreamState<'_, R>,
    line: Bytes,
) -> Result<(), String> {
    let Some(decoded) = decode_line(state, &line).await? else {
        return Ok(());
    };
    dispatch_decoded_line(state, decoded).await
}

async fn dispatch_decoded_line<R: Runtime>(
    state: &mut StreamState<'_, R>,
    decoded: DecodedLine,
) -> Result<(), String> {
    let DecodedLine { frame, permit } = decoded;
    emit_warning_logs(state.app, &frame);
    let topic_id = frame.topic_id.clone();
    if let Some(error) = frame.error {
        return send_error_result(&state.tx, topic_id, frame.legacy_attachment_warnings, error)
            .await;
    }
    validate_requested_message_ids(
        &topic_id,
        state
            .expected_messages
            .get(&topic_id)
            .and_then(Option::as_ref),
        &frame.messages,
    )?;
    if frame.messages.is_empty() {
        send_empty_result(&state.tx, topic_id, frame.legacy_attachment_warnings).await?;
        drop(permit);
    } else {
        spawn_topic_worker(
            &mut state.workers,
            permit,
            frame,
            state.app,
            state.write_queue,
            state.prerender_enabled,
            state.tx.clone(),
        );
    }
    Ok(())
}

async fn decode_line<R: Runtime>(
    state: &mut StreamState<'_, R>,
    line: &[u8],
) -> Result<Option<DecodedLine>, String> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let line_bytes = line.len();
    if line_bytes > super::MAX_NDJSON_LINE_BYTES {
        return Err("NDJSON frame exceeds 32MB budget".to_string());
    }
    let permit = state
        .sem
        .clone()
        .acquire_many_owned(pull_worker_permits(line_bytes)?)
        .await
        .map_err(|error| format!("Pull worker semaphore closed: {error}"))?;
    if let Some(error) = parse_stream_error_frame(line)? {
        return Err(error);
    }
    let frame = parse_topic_ndjson_frame(line)?;
    state
        .budget
        .observe_frame(line_bytes, frame.messages.len())?;
    validate_returned_topic_identity(&frame, state.expected_identities)?;
    if !state.seen_topics.insert(frame.topic_id.clone()) {
        return Err(format!(
            "NDJSON returned duplicate topicId {}",
            frame.topic_id
        ));
    }
    Ok(Some(DecodedLine { frame, permit }))
}
