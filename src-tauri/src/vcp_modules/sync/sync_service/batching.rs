use crate::vcp_modules::sync_pipeline::phase3_message::TopicLocalState;
use crate::vcp_modules::sync_types::{MessageDiffTopicState, MessageVersionState, OwnerType};
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;

pub(crate) const MAX_MESSAGES_PER_BATCH: usize = 10_000;
pub(crate) const MAX_WS_DIFF_BATCH_BYTES: usize = 8 * 1024 * 1024;
const DIFF_ENVELOPE_BYTES: usize = br#"{"type":"SYNC_MESSAGE_DIFF_REQUEST","topics":[]}"#.len();

pub(crate) struct Phase3DiffBatch {
    pub(crate) topics: Vec<MessageDiffTopicState>,
    pub(crate) keys: HashSet<TopicKey>,
}

pub(crate) type Phase3MessageSnapshots = HashMap<TopicKey, BTreeMap<String, MessageVersionState>>;

impl Phase3DiffBatch {
    pub(crate) fn message_snapshots(&self) -> Phase3MessageSnapshots {
        self.topics
            .iter()
            .map(|topic| {
                (
                    TopicKey::new(topic.owner_type.as_str(), &topic.owner_id, &topic.topic_id),
                    topic.messages.clone(),
                )
            })
            .collect()
    }
}

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

struct SizedTopic {
    key: TopicKey,
    state: MessageDiffTopicState,
    message_count: usize,
    bytes: usize,
}

fn size_topic(key: TopicKey, state: TopicLocalState) -> Result<SizedTopic, String> {
    let message_count = state.messages.len();
    if message_count > MAX_MESSAGES_PER_BATCH {
        return Err(format!(
            "Phase 3 diff topic {} exceeds the {MAX_MESSAGES_PER_BATCH}-message batch limit",
            key.topic_id
        ));
    }
    let owner_type = OwnerType::try_from(key.owner_type.as_str())
        .map_err(|_| format!("Phase 3 topic {} has invalid ownerType", key.topic_id))?;
    let topic = MessageDiffTopicState {
        owner_type,
        owner_id: key.owner_id.clone(),
        topic_id: key.topic_id.clone(),
        content_hash: state.content_hash,
        messages: state.messages,
    };
    let mut counter = JsonSizeCounter::new(MAX_WS_DIFF_BATCH_BYTES);
    serde_json::to_writer(&mut counter, &topic)
        .map_err(|error| format!("Failed to size Phase 3 topic {}: {error}", key.topic_id))?;
    if DIFF_ENVELOPE_BYTES.saturating_add(counter.bytes) > MAX_WS_DIFF_BATCH_BYTES {
        return Err(format!(
            "Phase 3 diff topic {} exceeds the 8 MiB WebSocket frame limit",
            key.topic_id
        ));
    }
    Ok(SizedTopic {
        key,
        state: topic,
        message_count,
        bytes: counter.bytes,
    })
}

fn should_flush(
    current: &Phase3DiffBatch,
    messages: usize,
    bytes: usize,
    next: &SizedTopic,
) -> bool {
    !current.topics.is_empty()
        && (messages.saturating_add(next.message_count) > MAX_MESSAGES_PER_BATCH
            || bytes.saturating_add(1).saturating_add(next.bytes) > MAX_WS_DIFF_BATCH_BYTES)
}

fn push_batch(queue: &mut VecDeque<Phase3DiffBatch>, batch: &mut Phase3DiffBatch) {
    if batch.topics.is_empty() {
        return;
    }
    queue.push_back(Phase3DiffBatch {
        topics: std::mem::take(&mut batch.topics),
        keys: std::mem::take(&mut batch.keys),
    });
}

pub(crate) fn build_diff_batches(
    topic_states: HashMap<TopicKey, TopicLocalState>,
) -> Result<VecDeque<Phase3DiffBatch>, String> {
    let mut ordered = topic_states.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut queue = VecDeque::new();
    let mut batch = Phase3DiffBatch {
        topics: Vec::new(),
        keys: HashSet::new(),
    };
    let mut messages = 0usize;
    let mut bytes = DIFF_ENVELOPE_BYTES;
    for (key, state) in ordered {
        let next = size_topic(key, state)?;
        if should_flush(&batch, messages, bytes, &next) {
            push_batch(&mut queue, &mut batch);
            messages = 0;
            bytes = DIFF_ENVELOPE_BYTES;
        }
        bytes = bytes
            .saturating_add(usize::from(!batch.topics.is_empty()))
            .saturating_add(next.bytes);
        messages = messages.saturating_add(next.message_count);
        batch.keys.insert(next.key);
        batch.topics.push(next.state);
    }
    push_batch(&mut queue, &mut batch);
    Ok(queue)
}
