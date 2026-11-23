use crate::vcp_modules::sync_types::MessageDeleteDecision;
use crate::vcp_modules::topic_types::TopicKey;
use serde::Serialize;
use std::io::Write;

pub(super) const MAX_NDJSON_LINE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_MESSAGE_ACK_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MESSAGE_REQUEST_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SYNC_TOPICS: usize = 10_000;
pub(super) const MAX_SYNC_MESSAGES: usize = 100_000;
pub(super) const MAX_MESSAGES_PER_TOPIC: usize = 10_000;
pub(super) const MESSAGE_PAGE_SIZE: usize = 100;
pub(super) const MAX_CONTROL_RESPONSE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_AVATAR_BYTES: usize = 20 * 1024 * 1024;

/// Wire 1.6 增量推送单个话题的消息与墓碑选择集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePushSelection {
    pub topic: TopicKey,
    pub message_ids: Vec<String>,
    pub deleted_messages: Vec<MessageDeleteDecision>,
}

/// 统一的推送目标：Wire 1.5 整话题上传，Wire 1.6 按差异选择集增量上传。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePushTarget {
    WholeTopic(TopicKey),
    Selected(MessagePushSelection),
}

impl MessagePushTarget {
    pub fn topic(&self) -> &TopicKey {
        match self {
            Self::WholeTopic(topic) => topic,
            Self::Selected(selection) => &selection.topic,
        }
    }
}

/// 批量 Push 单 Topic 的逐项结果。
#[derive(Debug)]
pub struct PushBatchResult {
    pub topic: TopicKey,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(super) struct MessageTombstone {
    pub(super) topic: TopicKey,
    pub(super) message_id: String,
    pub(super) deleted_at: i64,
}
pub(super) struct TopicMessagePreflight {
    pub(super) live_count: usize,
    pub(super) tombstone_count: usize,
}

#[derive(Debug)]
pub(super) struct SerializedTopicMessages {
    pub(super) line: Vec<u8>,
    pub(super) live_count: usize,
    pub(super) tombstone_count: usize,
}

pub(super) struct BoundedJsonLine {
    pub(super) bytes: Vec<u8>,
    limit: usize,
}

pub(super) struct CountingWriter {
    pub(super) bytes: usize,
    pub(super) limit: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("serialized byte count overflow"))?;
        if self.bytes > self.limit {
            return Err(std::io::Error::other(
                "serialized value exceeds byte budget",
            ));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[allow(dead_code)]
impl BoundedJsonLine {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
impl Write for BoundedJsonLine {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("JSON line exceeds its byte budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageTombstoneRequest<'a> {
    pub(super) msg_id: &'a str,
    pub(super) deleted_at: i64,
}

pub(super) struct MessagePushFrame {
    pub(super) outcome: PushBatchResult,
}
