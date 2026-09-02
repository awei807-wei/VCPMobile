use crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState;
use serde_json::{Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;

pub(crate) const MAX_MESSAGES_PER_BATCH: usize = 10_000;
pub(crate) const MAX_WS_DIFF_BATCH_BYTES: usize = 8 * 1024 * 1024;

struct JsonSizeCounter {
    bytes: usize,
    limit: usize,
}

impl JsonSizeCounter {
    fn new(limit: usize) -> Self {
        Self { bytes: 0, limit }
    }
}

impl Write for JsonSizeCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self.bytes.saturating_add(bytes.len());
        if next > self.limit {
            return Err(std::io::Error::other("JSON value exceeds its byte budget"));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn build_diff_batches(
    topic_states: HashMap<String, TopicLocalState>,
) -> Result<VecDeque<Map<String, Value>>, String> {
    let mut batches = VecDeque::new();
    let mut current_batch = Map::new();
    let mut current_msg_count = 0;
    let envelope_bytes = br#"{"type":"SYNC_MESSAGE_DIFF_BATCH","topics":{}}"#.len();
    let mut current_bytes = envelope_bytes;
    let mut topic_states = topic_states.into_iter().collect::<Vec<_>>();
    topic_states.sort_by(|left, right| left.0.cmp(&right.0));
    for (topic_id, state) in topic_states {
        let entry = build_topic_entry(&topic_id, state)?;
        let entry_bytes = entry.0;
        let msg_count = entry.1;
        if envelope_bytes.saturating_add(entry_bytes) > MAX_WS_DIFF_BATCH_BYTES {
            return Err(format!(
                "Phase 3 diff topic {topic_id} exceeds the 8 MiB WebSocket frame limit"
            ));
        }
        let separator = usize::from(!current_batch.is_empty());
        if should_start_new_batch(
            current_batch.is_empty(),
            current_msg_count,
            msg_count,
            current_bytes,
            separator,
            entry_bytes,
        ) {
            batches.push_back(current_batch);
            current_batch = Map::new();
            current_msg_count = 0;
            current_bytes = envelope_bytes;
        }
        current_bytes = current_bytes
            .saturating_add(usize::from(!current_batch.is_empty()))
            .saturating_add(entry_bytes);
        current_batch.insert(topic_id, entry.2);
        current_msg_count = current_msg_count.saturating_add(msg_count);
    }
    if !current_batch.is_empty() {
        batches.push_back(current_batch);
    }
    Ok(batches)
}

fn build_topic_entry(
    topic_id: &str,
    state: TopicLocalState,
) -> Result<(usize, usize, Value), String> {
    let msg_count = state.messages.len();
    if msg_count > MAX_MESSAGES_PER_BATCH {
        return Err(format!(
            "Phase 3 diff topic {topic_id} exceeds the {MAX_MESSAGES_PER_BATCH}-message batch limit"
        ));
    }
    let mut msg_map = Map::new();
    let mut messages = state.messages.into_iter().collect::<Vec<_>>();
    messages.sort_by(|left, right| left.0.cmp(&right.0));
    for (message_id, hash) in messages {
        msg_map.insert(message_id, Value::String(hash));
    }
    let topic_obj = serde_json::json!({
        "ownerType": state.owner_type,
        "ownerId": state.owner_id,
        "topicHash": state.topic_hash,
        "messages": msg_map,
    });
    let mut counter = JsonSizeCounter::new(MAX_WS_DIFF_BATCH_BYTES);
    serde_json::to_writer(&mut counter, &topic_id)
        .and_then(|_| counter.write_all(b":").map_err(serde_json::Error::io))
        .and_then(|_| serde_json::to_writer(&mut counter, &topic_obj))
        .map_err(|error| format!("Failed to size Phase 3 topic {topic_id}: {error}"))?;
    Ok((counter.bytes, msg_count, topic_obj))
}

fn should_start_new_batch(
    current_empty: bool,
    current_msg_count: usize,
    msg_count: usize,
    current_bytes: usize,
    separator: usize,
    entry_bytes: usize,
) -> bool {
    !current_empty
        && (current_msg_count.saturating_add(msg_count) > MAX_MESSAGES_PER_BATCH
            || current_bytes
                .saturating_add(separator)
                .saturating_add(entry_bytes)
                > MAX_WS_DIFF_BATCH_BYTES)
}
