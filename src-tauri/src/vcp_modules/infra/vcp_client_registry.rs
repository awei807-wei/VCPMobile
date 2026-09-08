use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use dashmap::{mapref::entry::Entry, DashMap};
use std::sync::{atomic::AtomicU64, Arc, Weak};
use tokio::sync::oneshot;

#[path = "vcp_client_registry_lease.rs"]
mod lease;
pub use lease::{CompletionLease, GuardedTransition};

#[path = "vcp_client_registry_identity.rs"]
mod identity;
#[allow(unused_imports)]
pub use identity::owner_key_from_parts;
pub use identity::{message_key_from_context, message_key_from_parts, optional_message_key};

#[path = "vcp_client_registry_generation.rs"]
mod generation;

#[path = "vcp_client_registry_query.rs"]
mod query;
pub use query::ActiveRequestQuery;

#[path = "vcp_client_registry/locks.rs"]
mod locks;
pub(super) use locks::{TopicTransitionLock, TransitionLock};

#[path = "vcp_client_registry/operations.rs"]
mod operations;

/// 恢复流程申请当前消息身份时的结果。
pub enum ClaimIfInactive {
    Claimed(CompletionLease),
    Active { generation: Option<u64>, epoch: u64 },
}

pub type RecoveryClaim = ClaimIfInactive;

/// 请求条目按完整消息身份隔离，纪元防止旧流清理重试后的新请求。
struct ActiveRequestEntry {
    sender: oneshot::Sender<()>,
    epoch: u64,
    session_generation: Option<u64>,
}

/// Thread-safe registry for cancellable VCP requests.
#[derive(Clone)]
pub struct ActiveRequestRegistry {
    inner: Arc<ActiveRequestRegistryInner>,
}

struct ActiveRequestRegistryInner {
    entries: DashMap<MessageKey, ActiveRequestEntry>,
    current_epochs: DashMap<MessageKey, u64>,
    transitions: DashMap<MessageKey, Weak<TransitionLock>>,
    topic_transitions: DashMap<TopicKey, Weak<TopicTransitionLock>>,
    next_epoch: AtomicU64,
}

impl Default for ActiveRequestRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(ActiveRequestRegistryInner {
                entries: DashMap::new(),
                current_epochs: DashMap::new(),
                transitions: DashMap::new(),
                topic_transitions: DashMap::new(),
                next_epoch: AtomicU64::new(0),
            }),
        }
    }
}

/// Public state wrapper retained for Tauri and existing call sites.
pub struct ActiveRequests(pub Arc<ActiveRequestRegistry>);

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(ActiveRequestRegistry::default()))
    }
}

#[cfg(test)]
#[path = "vcp_client_registry_tests.rs"]
mod tests;
