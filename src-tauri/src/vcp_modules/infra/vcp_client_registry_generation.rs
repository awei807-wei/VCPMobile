use super::{ActiveRequestRegistry, GuardedTransition, MessageKey};
use std::future::Future;

impl ActiveRequestRegistry {
    /// 将原生 helper 返回的真实 session generation 绑定到当前请求。
    /// 旧请求或已替换请求不能写入新 generation。
    pub fn bind_session_generation(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        session_generation: u64,
    ) -> bool {
        if session_generation == 0 {
            return false;
        }
        let Some(mut entry) = self.inner.entries.get_mut(key) else {
            return false;
        };
        if entry.epoch != request_epoch {
            return false;
        }
        entry.session_generation = Some(session_generation);
        true
    }

    /// 在当前请求的转换锁内持久化并绑定 helper generation。
    ///
    /// 持久化回调在锁内等待，故同一消息的新请求不能在数据库写入和
    /// registry 绑定之间穿过；旧 epoch 只会得到 `Skipped`，绝不把
    /// generation 写到替换后的请求上。
    pub async fn bind_session_generation_with<T, F, Fut>(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        session_generation: u64,
        persist: F,
    ) -> Result<GuardedTransition<T>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        if session_generation == 0 {
            return Err("helper generation 必须是正整数".to_string());
        }
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(key);
        let _transition_guard = transition.lock().await;
        let current = self
            .inner
            .entries
            .get(key)
            .is_some_and(|entry| entry.epoch == request_epoch);
        if !current {
            return Ok(GuardedTransition::Skipped);
        }
        let result = persist().await?;
        let Some(mut entry) = self.inner.entries.get_mut(key) else {
            return Ok(GuardedTransition::Skipped);
        };
        if entry.epoch != request_epoch {
            return Ok(GuardedTransition::Skipped);
        }
        entry.session_generation = Some(session_generation);
        Ok(GuardedTransition::Applied(result))
    }

    /// 在同一 key transition 内观察并复核当前 registry 的 helper generation。
    ///
    /// helper 查询、数据库校验和查询后的 registry 复核必须共享这把锁，
    /// 否则 replacement 可能在三方状态之间穿过并被旧恢复结果误用。
    pub async fn with_active_generation<T, F, Fut>(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        helper_generation: u64,
        operation: F,
    ) -> Result<GuardedTransition<T>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        if helper_generation == 0 {
            return Err("helper generation 必须是正整数".to_string());
        }
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(key);
        let _transition_guard = transition.lock().await;
        if !self.is_active_generation(key, request_epoch, helper_generation) {
            return Ok(GuardedTransition::Skipped);
        }
        let result = operation().await?;
        if !self.is_active_generation(key, request_epoch, helper_generation) {
            return Ok(GuardedTransition::Skipped);
        }
        Ok(GuardedTransition::Applied(result))
    }

    /// 在同一消息转换锁内授权 helper generation stop。
    ///
    /// 用户取消或终态清理可能已经先移除了旧请求，因此“当前条目不存在”
    /// 仍允许 stop。唯一禁止的情况是当前条目已经被登记为同一 helper
    /// generation 的新 Rust epoch；此时旧消费者只应断开自己的 socket。
    /// 实际 stop 操作在锁内执行，避免检查通过后被并发 resume 替换。
    pub async fn with_helper_stop_authorization<T, F, Fut>(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        helper_generation: u64,
        operation: F,
    ) -> Result<GuardedTransition<T>, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        if helper_generation == 0 {
            return Err("helper generation 必须是正整数".to_string());
        }
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(key);
        let _transition_guard = transition.lock().await;
        if self.is_replaced_with_same_generation(key, request_epoch, helper_generation) {
            return Ok(GuardedTransition::Skipped);
        }
        operation().await.map(GuardedTransition::Applied)
    }

    /// 快速判断旧消费者是否已被同 helper generation 的请求接管。
    /// 注册接管目标时 generation 与新 epoch 在同一转换锁内写入，故在旧
    /// 消费者收到其取消信号后，该观察不会把普通用户取消误判为迁移。
    pub fn is_same_helper_generation_takeover(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        helper_generation: u64,
    ) -> bool {
        helper_generation > 0
            && self.is_replaced_with_same_generation(key, request_epoch, helper_generation)
    }

    fn is_replaced_with_same_generation(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        helper_generation: u64,
    ) -> bool {
        self.inner.entries.get(key).is_some_and(|entry| {
            entry.epoch != request_epoch && entry.session_generation == Some(helper_generation)
        })
    }

    fn is_active_generation(
        &self,
        key: &MessageKey,
        request_epoch: u64,
        helper_generation: u64,
    ) -> bool {
        self.inner.entries.get(key).is_some_and(|entry| {
            entry.epoch == request_epoch && entry.session_generation == Some(helper_generation)
        })
    }
}
