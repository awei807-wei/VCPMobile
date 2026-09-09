use serde::Serialize;
use serde_json::Value;

/// Hashing and canonicalization shared by every Wire 1.4 sync phase.
pub struct HashAggregator;

const DEFAULT_INVITE_PROMPT: &str = "现在轮到你{{VCPChatAgentName}}发言了。系统已经为大家添加[xxx的发言：]这样的标记头，以用于区分不同发言来自谁。大家不用自己再输出自己的发言标记头，也不需要讨论发言标记系统，正常聊天即可。";

#[path = "sync_hash/bubble_hash.rs"]
mod bubble_hash;
#[path = "sync_hash/canonical.rs"]
mod canonical;
#[path = "sync_hash/config_hash.rs"]
mod config_hash;
#[path = "sync_hash/merkle_hash.rs"]
mod merkle_hash;
#[path = "sync_hash/message_hash.rs"]
mod message_hash;

pub fn canonical_json(value: &Value) -> String {
    canonical::canonical_json(value)
}

pub fn compute_canonical_hash<T: Serialize>(data: &T) -> String {
    canonical::compute_canonical_hash(data)
}

#[path = "sync_hash_initializer.rs"]
mod initializer;
pub use initializer::HashInitializer;

#[cfg(test)]
#[path = "sync_hash_tests.rs"]
mod tests;
