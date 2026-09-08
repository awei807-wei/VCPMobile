use super::{ActiveRequestRegistryInner, MessageKey, TopicKey};
use dashmap::mapref::entry::Entry;
use std::sync::Weak;
use tokio::sync::Mutex;

/// Per-message transition lock used to serialize registration and finalization.
pub struct TransitionLock {
    pub(super) key: MessageKey,
    pub(super) registry: Weak<ActiveRequestRegistryInner>,
    pub(super) mutex: Mutex<()>,
}

impl TransitionLock {
    pub(super) fn new(key: MessageKey, registry: Weak<ActiveRequestRegistryInner>) -> Self {
        Self {
            key,
            registry,
            mutex: Mutex::new(()),
        }
    }

    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mutex.lock().await
    }
}

impl Drop for TransitionLock {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        if let Entry::Occupied(entry) = registry.transitions.entry(self.key.clone()) {
            if entry.get().as_ptr() == self as *const TransitionLock {
                entry.remove();
            }
        };
    }
}

/// Topic-wide mutation barrier. Every request registration and guarded
/// persistence operation acquires this barrier before its message lock so a
/// mutation cannot commit between registry registration and skeleton writes.
pub struct TopicTransitionLock {
    pub(super) topic: TopicKey,
    pub(super) registry: Weak<ActiveRequestRegistryInner>,
    pub(super) mutex: Mutex<()>,
}

impl TopicTransitionLock {
    pub(super) fn new(topic: TopicKey, registry: Weak<ActiveRequestRegistryInner>) -> Self {
        Self {
            topic,
            registry,
            mutex: Mutex::new(()),
        }
    }

    pub(super) async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mutex.lock().await
    }
}

impl Drop for TopicTransitionLock {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        if let Entry::Occupied(entry) = registry.topic_transitions.entry(self.topic.clone()) {
            if entry.get().as_ptr() == self as *const TopicTransitionLock {
                entry.remove();
            }
        };
    }
}
