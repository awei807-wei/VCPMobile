use super::frame_validation::{
    parse_topic_ndjson_frame, validate_requested_message_ids, validate_returned_topic_identity,
    ExpectedMessages,
};
use super::message_persistence::process_topic_messages;
use super::ndjson_codec::{parse_stream_error_frame, NdjsonBudget};
use super::result_reporting::{emit_progress, emit_warning_logs, topic_result};
use super::{BatchPullResult, PullProgressContext};
use crate::vcp_modules::db_write_queue::{DbWriteQueue, ExpectedMessageStates};
use crate::vcp_modules::sync_types::MessageVersionState;
use crate::vcp_modules::topic_types::TopicKey;
use futures_util::{Stream, StreamExt};
use std::collections::{BTreeMap, HashMap, HashSet};
use tauri::{AppHandle, Runtime};
use tokio_util::bytes::{Bytes, BytesMut};

pub(crate) async fn consume_stream<R: Runtime>(
    app: &AppHandle<R>,
    response: reqwest::Response,
    expected_messages: &ExpectedMessages,
    expected_local_states: Option<&HashMap<TopicKey, BTreeMap<String, MessageVersionState>>>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
    progress: Option<PullProgressContext>,
) -> Result<Vec<BatchPullResult>, String> {
    let mut state = StreamState {
        app,
        expected_messages,
        expected_local_states,
        write_queue,
        prerender_enabled,
        progress,
        budget: NdjsonBudget::new(expected_messages.len()),
        buffer: BytesMut::new(),
        seen_topics: HashSet::with_capacity(expected_messages.len()),
        results: Vec::with_capacity(expected_messages.len()),
    };
    let mut stream = response.bytes_stream();
    consume_chunks(&mut stream, &mut state).await?;
    consume_trailing_line(&mut state).await?;
    ensure_expected_topics(expected_messages, &state.seen_topics)?;
    Ok(state.results)
}

/// Byte-oriented NDJSON splitter. UTF-8 is decoded only after a complete line
/// is assembled, so a multi-byte character split across HTTP chunks cannot be
/// corrupted or accidentally decoded with replacement characters.
#[derive(Default)]
pub(crate) struct NdjsonLineBuffer {
    buffer: BytesMut,
}

impl NdjsonLineBuffer {
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Bytes>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.split_to(position + 1).freeze();
            if line.len() > super::MAX_NDJSON_LINE_BYTES {
                return Err("NDJSON frame exceeds 32 MiB budget".to_string());
            }
            lines.push(line);
        }
        if self.buffer.len() > super::MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32 MiB budget".to_string());
        }
        Ok(lines)
    }

    pub(crate) fn finish(&mut self) -> Result<Option<Bytes>, String> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        if self.buffer.len() > super::MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32 MiB budget".to_string());
        }
        Ok(Some(self.buffer.split_to(self.buffer.len()).freeze()))
    }
}

struct StreamState<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    expected_messages: &'a ExpectedMessages,
    expected_local_states: Option<&'a HashMap<TopicKey, BTreeMap<String, MessageVersionState>>>,
    write_queue: &'a DbWriteQueue,
    prerender_enabled: bool,
    progress: Option<PullProgressContext>,
    budget: NdjsonBudget,
    buffer: BytesMut,
    seen_topics: HashSet<TopicKey>,
    results: Vec<BatchPullResult>,
}

async fn consume_chunks<S, R>(stream: &mut S, state: &mut StreamState<'_, R>) -> Result<(), String>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
    R: Runtime,
{
    while let Some(chunk_result) = tokio::time::timeout(super::NDJSON_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| "NDJSON stream idle timeout after 30 seconds".to_string())?
    {
        let chunk = chunk_result.map_err(|error| format!("NDJSON stream read error: {error}"))?;
        state.budget.observe_chunk(chunk.len())?;
        state.buffer.extend_from_slice(&chunk);
        consume_complete_lines(state).await?;
        if state.buffer.len() > super::MAX_NDJSON_LINE_BYTES {
            return Err("NDJSON frame exceeds 32 MiB budget".to_string());
        }
    }
    Ok(())
}

async fn consume_complete_lines<R: Runtime>(state: &mut StreamState<'_, R>) -> Result<(), String> {
    while let Some(position) = state.buffer.iter().position(|byte| *byte == b'\n') {
        let line = state.buffer.split_to(position + 1).freeze();
        consume_line(state, line).await?;
    }
    Ok(())
}

async fn consume_trailing_line<R: Runtime>(state: &mut StreamState<'_, R>) -> Result<(), String> {
    if state.buffer.is_empty() {
        return Ok(());
    }
    let line = state.buffer.split_to(state.buffer.len()).freeze();
    consume_line(state, line).await
}

async fn consume_line<R: Runtime>(
    state: &mut StreamState<'_, R>,
    line: Bytes,
) -> Result<(), String> {
    let Some(frame) = decode_topic_line(state, &line)? else {
        return Ok(());
    };
    emit_warning_logs(state.app, &frame);
    if let Some(error) = frame.error {
        state.results.push(topic_result(
            frame.topic,
            false,
            0,
            0,
            frame.legacy_attachment_warnings,
            Some(crate::vcp_modules::sync::sync_error::encode_wire_sync_error(&error)?),
        ));
        return Ok(());
    }
    persist_topic(state, frame).await
}

fn decode_topic_line<R: Runtime>(
    state: &mut StreamState<'_, R>,
    line: &Bytes,
) -> Result<Option<super::frame_validation::TopicNDJSONFrame>, String> {
    if line.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(None);
    }
    if line.len() > super::MAX_NDJSON_LINE_BYTES {
        return Err("NDJSON frame exceeds 32 MiB budget".to_string());
    }
    let line_without_newline: &[u8] = if line.last() == Some(&b'\n') {
        &line[..line.len() - 1]
    } else {
        line
    };
    if let Some(error) = parse_stream_error_frame(line_without_newline)? {
        return Err(error);
    }
    let frame = parse_topic_ndjson_frame(line_without_newline)?;
    state
        .budget
        .observe_frame(line.len(), frame.messages.len())?;
    let expected_topics = state
        .expected_messages
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    validate_returned_topic_identity(&frame, &expected_topics)?;
    if !state.seen_topics.insert(frame.topic.clone()) {
        return Err(format!(
            "NDJSON returned duplicate topic identity {}/{}/{}",
            frame.topic.owner_type, frame.topic.owner_id, frame.topic.topic_id
        ));
    }
    Ok(Some(frame))
}

async fn persist_topic<R: Runtime>(
    state: &mut StreamState<'_, R>,
    frame: super::frame_validation::TopicNDJSONFrame,
) -> Result<(), String> {
    let expected = state
        .expected_messages
        .get(&frame.topic)
        .ok_or_else(|| "NDJSON returned topic identity disappeared".to_string())?;
    validate_requested_message_ids(&frame.topic, expected.as_ref(), &frame.messages)?;
    let topic = frame.topic.clone();
    let warning_count = frame.legacy_attachment_warnings;
    let expected_states =
        expected_message_states(&frame.topic, &frame.messages, state.expected_local_states)?;
    match process_topic_messages(
        state.app,
        &topic,
        frame.messages,
        expected_states,
        state.write_queue,
        state.prerender_enabled,
    )
    .await
    {
        Ok((parsed_count, failed_count)) => {
            state.results.push(topic_result(
                topic,
                true,
                parsed_count,
                failed_count,
                warning_count,
                None,
            ));
            emit_progress(state.app, state.progress.as_ref(), state.results.len());
        }
        Err(error) => {
            state
                .results
                .push(topic_result(topic, false, 0, 0, warning_count, Some(error)))
        }
    }
    Ok(())
}

fn expected_message_states(
    topic: &TopicKey,
    messages: &[crate::vcp_modules::sync_dto::MessageSyncDTO],
    snapshots: Option<&HashMap<TopicKey, BTreeMap<String, MessageVersionState>>>,
) -> Result<Option<ExpectedMessageStates>, String> {
    let Some(snapshots) = snapshots else {
        return Ok(None);
    };
    let snapshot = snapshots
        .get(topic)
        .ok_or_else(|| "Phase 3 local snapshot is missing a pulled topic".to_string())?;
    Ok(Some(
        messages
            .iter()
            .map(|message| (message.id.clone(), snapshot.get(&message.id).cloned()))
            .collect(),
    ))
}

fn ensure_expected_topics(
    expected_messages: &ExpectedMessages,
    seen_topics: &HashSet<TopicKey>,
) -> Result<(), String> {
    if expected_messages
        .keys()
        .all(|topic| seen_topics.contains(topic))
    {
        return Ok(());
    }
    let mut missing = expected_messages
        .keys()
        .filter(|topic| !seen_topics.contains(*topic))
        .map(|topic| format!("{}/{}/{}", topic.owner_type, topic.owner_id, topic.topic_id))
        .collect::<Vec<_>>();
    missing.sort();
    Err(format!("NDJSON response is missing topics {missing:?}"))
}
