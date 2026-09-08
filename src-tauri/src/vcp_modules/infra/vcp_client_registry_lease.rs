use super::{ActiveRequestRegistry, MessageKey, TopicTransitionLock, TransitionLock};
use std::sync::Arc;

/// 受保护操作的结果。请求被替换时跳过旧请求，不将其视为持久化错误。
#[derive(Debug, PartialEq, Eq)]
pub enum GuardedTransition<T> {
    Applied(T),
    Skipped,
}

struct CompletionLeaseInner {
    registry: ActiveRequestRegistry,
    key: MessageKey,
    epoch: u64,
    transition: Arc<TransitionLock>,
    topic_transition: Arc<TopicTransitionLock>,
}

/// 网络完成后继续持有消息身份，直到调用方完成受保护的持久化和事件转换。
#[derive(Clone)]
pub struct CompletionLease {
    inner: Arc<CompletionLeaseInner>,
}

impl CompletionLease {
    pub(super) fn new(
        registry: ActiveRequestRegistry,
        key: MessageKey,
        epoch: u64,
        transition: Arc<TransitionLock>,
        topic_transition: Arc<TopicTransitionLock>,
    ) -> Self {
        Self {
            inner: Arc::new(CompletionLeaseInner {
                registry,
                key,
                epoch,
                transition,
                topic_transition,
            }),
        }
    }

    /// 返回租约绑定的完整消息身份。
    pub fn key(&self) -> &MessageKey {
        &self.inner.key
    }

    /// 返回租约对应的请求纪元。
    pub fn epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// 仅在请求仍为当前 epoch 时执行操作，并在所有 await 期间持有每 key 锁。
    pub async fn with_current_transition<T, F, Fut>(
        &self,
        operation: F,
    ) -> Result<GuardedTransition<T>, String>
    where
        F: FnOnce(MessageKey) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        // Keep lock order consistent with registration and topic mutations:
        // topic barrier first, then the message transition.
        let _topic_guard = self.inner.topic_transition.lock().await;
        let _transition_guard = self.inner.transition.lock().await;
        if !self
            .inner
            .registry
            .is_current_epoch(&self.inner.key, self.inner.epoch)
        {
            return Ok(GuardedTransition::Skipped);
        }
        operation(self.inner.key.clone())
            .await
            .map(GuardedTransition::Applied)
    }
}

impl Drop for CompletionLeaseInner {
    fn drop(&mut self) {
        self.registry
            .release_lease(&self.key, self.epoch, &self.transition);
    }
}
