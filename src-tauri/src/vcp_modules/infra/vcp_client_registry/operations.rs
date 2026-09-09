use super::{
    ActiveRequestEntry, ActiveRequestQuery, ActiveRequestRegistry, ClaimIfInactive, Entry,
    MessageKey, TopicKey, TopicTransitionLock, TransitionLock,
};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use tokio::sync::oneshot;

impl ActiveRequestRegistry {
    /// 在终结器使用的同一把每 key 锁下注册完整请求身份。
    pub async fn register(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
    ) -> (u64, Option<oneshot::Sender<()>>, super::CompletionLease) {
        self.register_for_key(key, sender).await
    }

    /// 注册请求并返回中止纪元、旧请求发送端和完成租约。
    /// 活动中止条目移除后，租约仍保持有效，直到持久化调用方释放。
    pub async fn register_for_key(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
    ) -> (u64, Option<oneshot::Sender<()>>, super::CompletionLease) {
        self.register_with_session_generation(key, sender, None)
            .await
    }

    /// 注册一个要迁移到既有 helper session 的请求。
    ///
    /// 这个注册必须在返回旧请求的取消发送端之前写入目标 generation，
    /// 这样旧消费者收到取消信号后能原子地识别为“只迁移 socket”，而
    /// 不是把同一个 helper generation 一并 stop 掉。
    pub async fn register_for_generation(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
        session_generation: u64,
    ) -> Result<(u64, Option<oneshot::Sender<()>>, super::CompletionLease), String> {
        if session_generation == 0 {
            return Err("helper generation 必须是正整数".to_string());
        }
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(&key);
        let _transition_guard = transition.lock().await;
        if let Some(entry) = self.inner.entries.get(&key) {
            if entry.session_generation != Some(session_generation) {
                return Err(format!(
                    "活动请求 helper generation 不匹配: expected={session_generation}, actual={:?}",
                    entry.session_generation
                ));
            }
        }
        let epoch = self.next_epoch();
        self.inner.current_epochs.insert(key.clone(), epoch);
        let previous = self.inner.entries.insert(
            key.clone(),
            ActiveRequestEntry {
                sender,
                epoch,
                session_generation: Some(session_generation),
            },
        );
        drop(_transition_guard);
        drop(_topic_guard);
        let lease =
            super::CompletionLease::new(self.clone(), key, epoch, transition, topic_transition);
        Ok((epoch, previous.map(|entry| entry.sender), lease))
    }

    async fn register_with_session_generation(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
        session_generation: Option<u64>,
    ) -> (u64, Option<oneshot::Sender<()>>, super::CompletionLease) {
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(&key);
        let _transition_guard = transition.lock().await;
        let epoch = self.next_epoch();
        self.inner.current_epochs.insert(key.clone(), epoch);
        let previous = self.inner.entries.insert(
            key.clone(),
            ActiveRequestEntry {
                sender,
                epoch,
                session_generation: session_generation
                    .or_else(|| session_generation_for_new_request(epoch)),
            },
        );
        drop(_transition_guard);
        drop(_topic_guard);
        let lease =
            super::CompletionLease::new(self.clone(), key, epoch, transition, topic_transition);
        (epoch, previous.map(|entry| entry.sender), lease)
    }

    /// 移除完整 key，并返回对应的取消发送端。
    pub fn remove_key(&self, key: &MessageKey) -> Option<(MessageKey, oneshot::Sender<()>)> {
        self.inner.entries.remove(key).map(|(key, entry)| {
            self.inner
                .current_epochs
                .remove_if(&key, |_key, current_epoch| *current_epoch == entry.epoch);
            (key, entry.sender)
        })
    }

    /// 仅当条目仍属于指定注册纪元时移除。
    pub fn remove_if_current(&self, key: &MessageKey, epoch: u64) -> bool {
        let removed = self
            .inner
            .entries
            .remove_if(key, |_key, entry| entry.epoch == epoch)
            .is_some();
        let invalidated = self
            .inner
            .current_epochs
            .remove_if(key, |_key, current_epoch| *current_epoch == epoch)
            .is_some();
        removed || invalidated
    }

    /// Remove only the cancellable sender for a completed request.
    ///
    /// The current epoch deliberately remains until the caller drops its
    /// completion lease/guard, so a success or error finalizer can still pass
    /// `with_current_transition` after the transport has finished.
    pub fn remove_entry_if_current(&self, key: &MessageKey, epoch: u64) -> bool {
        self.inner
            .entries
            .remove_if(key, |_key, entry| entry.epoch == epoch)
            .is_some()
    }

    /// 捕获指定话题当前活动请求的完整消息 key 和 registry epoch。
    ///
    /// `current_epochs` 是请求注册和恢复 claim 的共同身份账本；不能只
    /// 扫描 `entries`，因为恢复 claim 在尚未绑定可取消 sender 时也必须
    /// 能被话题编辑屏障捕获和取消。
    pub fn snapshot_epochs_for_topic(&self, topic: &TopicKey) -> Vec<(MessageKey, u64)> {
        let mut snapshot = self
            .inner
            .current_epochs
            .iter()
            .filter(|entry| entry.key().topic == *topic)
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect::<Vec<_>>();

        // Keep a defensive fallback for a transient/legacy entry that has not
        // yet been reflected in the epoch ledger. Prefer the ledger when
        // both maps contain the same key so a stale entry cannot resurrect an
        // older epoch in a mutation capture.
        for entry in self.inner.entries.iter() {
            if entry.key().topic == *topic && snapshot.iter().all(|(key, _)| key != entry.key()) {
                snapshot.push((entry.key().clone(), entry.epoch));
            }
        }
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        snapshot
    }

    /// 仅在请求仍是捕获时的同一 epoch 时发送取消信号并移除它。
    pub async fn cancel_if_current(&self, key: &MessageKey, epoch: u64) -> bool {
        let transition = self.transition_for(key);
        let (removed, invalidated) = {
            let _transition_guard = transition.lock().await;
            let removed = self
                .inner
                .entries
                .remove_if(key, |_key, entry| entry.epoch == epoch)
                .map(|(_, entry)| entry);
            let invalidated = self
                .inner
                .current_epochs
                .remove_if(key, |_key, current_epoch| *current_epoch == epoch)
                .is_some();
            (removed, invalidated)
        };
        self.cleanup_transition(key, &transition);
        if let Some(entry) = removed {
            let _ = entry.sender.send(());
            true
        } else {
            // A recovery claim has a current epoch but no ActiveRequestEntry;
            // cancellation still has to invalidate that epoch so its guarded
            // finalizer cannot commit after the edit barrier.
            invalidated
        }
    }

    /// 与同一消息 key 的待处理终结器串行移除活动中止条目。
    pub async fn remove_key_guarded(
        &self,
        key: &MessageKey,
    ) -> Option<(MessageKey, oneshot::Sender<()>)> {
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(key);
        let result = {
            let _transition_guard = transition.lock().await;
            self.remove_key(key)
        };
        self.cleanup_transition(key, &transition);
        result
    }

    /// 在每 key 转换锁下移除完整 key，或唯一匹配的旧版消息 ID。
    pub async fn remove_guarded<Q>(&self, query: &Q) -> Option<(MessageKey, oneshot::Sender<()>)>
    where
        Q: ActiveRequestQuery + ?Sized,
    {
        let key = query.resolve(self)?;
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(&key);
        let result = {
            let _transition_guard = transition.lock().await;
            if query.resolve(self).as_ref() != Some(&key) {
                None
            } else {
                self.remove_key(&key)
            }
        };
        self.cleanup_transition(&key, &transition);
        result
    }

    /// 在同一消息转换锁下检查活动请求，并为恢复流程原子地创建租约。
    pub async fn claim_if_inactive(&self, key: MessageKey) -> Result<ClaimIfInactive, String> {
        let topic_transition = self.topic_transition_for(&key.topic);
        let _topic_guard = topic_transition.lock().await;
        let transition = self.transition_for(&key);
        let _transition_guard = transition.lock().await;
        if let Some(entry) = self.inner.entries.get(&key) {
            return Ok(ClaimIfInactive::Active {
                generation: entry.session_generation,
                epoch: entry.epoch,
            });
        }
        if let Some(epoch) = self.inner.current_epochs.get(&key).map(|value| *value) {
            return Ok(ClaimIfInactive::Active {
                generation: None,
                epoch,
            });
        }
        let epoch = self.next_epoch();
        self.inner.current_epochs.insert(key.clone(), epoch);
        drop(_transition_guard);
        drop(_topic_guard);
        Ok(ClaimIfInactive::Claimed(super::CompletionLease::new(
            self.clone(),
            key,
            epoch,
            transition,
            topic_transition,
        )))
    }

    /// Serialize a topic-scoped mutation with request registration and all
    /// guarded request persistence. The callback runs while the barrier is
    /// held, so captured epochs can be cancelled before a waiting old lease
    /// reaches its message transition.
    pub async fn with_topic_mutation<T, F, Fut>(
        &self,
        topic: &TopicKey,
        operation: F,
    ) -> Result<T, String>
    where
        F: FnOnce(Vec<(MessageKey, u64)>) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let topic_transition = self.topic_transition_for(topic);
        let _topic_guard = topic_transition.lock().await;
        let captured = self.snapshot_epochs_for_topic(topic);
        operation(captured).await
    }

    /// Remove by an exact composite key, or by a legacy message id only when
    /// exactly one active request has that id. Ambiguous legacy lookup fails
    /// closed and returns `None`.
    pub fn remove<Q>(&self, query: &Q) -> Option<(MessageKey, oneshot::Sender<()>)>
    where
        Q: ActiveRequestQuery + ?Sized,
    {
        query.resolve(self).and_then(|key| self.remove_key(&key))
    }

    /// Check an exact key, or an unambiguous legacy message id.
    pub fn contains_key<Q>(&self, query: &Q) -> bool
    where
        Q: ActiveRequestQuery + ?Sized,
    {
        query.resolve(self).is_some_and(|key| {
            self.inner.entries.contains_key(&key) || self.inner.current_epochs.contains_key(&key)
        })
    }

    pub fn len(&self) -> usize {
        self.inner.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.entries.is_empty()
    }

    pub(super) fn next_epoch(&self) -> u64 {
        self.inner
            .next_epoch
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    pub(super) fn transition_for(&self, key: &MessageKey) -> Arc<TransitionLock> {
        let candidate = Arc::new(TransitionLock::new(
            key.clone(),
            Arc::downgrade(&self.inner),
        ));
        let candidate_weak = Arc::downgrade(&candidate);
        match self.inner.transitions.entry(key.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(candidate_weak);
                candidate
            }
            Entry::Occupied(mut entry) => {
                if let Some(transition) = entry.get().upgrade() {
                    transition
                } else {
                    entry.insert(candidate_weak);
                    candidate
                }
            }
        }
    }

    pub(super) fn topic_transition_for(&self, topic: &TopicKey) -> Arc<TopicTransitionLock> {
        let candidate = Arc::new(TopicTransitionLock::new(
            topic.clone(),
            Arc::downgrade(&self.inner),
        ));
        let candidate_weak = Arc::downgrade(&candidate);
        match self.inner.topic_transitions.entry(topic.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(candidate_weak);
                candidate
            }
            Entry::Occupied(mut entry) => {
                if let Some(transition) = entry.get().upgrade() {
                    transition
                } else {
                    entry.insert(candidate_weak);
                    candidate
                }
            }
        }
    }

    pub(super) fn release_lease(
        &self,
        key: &MessageKey,
        epoch: u64,
        transition: &Arc<TransitionLock>,
    ) {
        self.inner
            .current_epochs
            .remove_if(key, |_key, current_epoch| *current_epoch == epoch);
        self.cleanup_transition(key, transition);
    }

    pub(super) fn cleanup_transition(&self, key: &MessageKey, transition: &Arc<TransitionLock>) {
        let expected = Arc::downgrade(transition);
        if let Entry::Occupied(entry) = self.inner.transitions.entry(key.clone()) {
            if Weak::ptr_eq(entry.get(), &expected) && Arc::strong_count(transition) == 1 {
                entry.remove();
            }
        }
    }

    pub(super) fn is_current_epoch(&self, key: &MessageKey, epoch: u64) -> bool {
        self.inner
            .current_epochs
            .get(key)
            .is_some_and(|current| *current == epoch)
    }
}

fn session_generation_for_new_request(_request_epoch: u64) -> Option<u64> {
    None
}
