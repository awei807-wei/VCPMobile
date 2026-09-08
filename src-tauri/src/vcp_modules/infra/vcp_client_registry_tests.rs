use super::{
    message_key_from_context, message_key_from_parts, ActiveRequestRegistry, ClaimIfInactive,
    GuardedTransition,
};
use crate::vcp_modules::chat::topic_types::MessageKey;
use std::sync::Arc;
use tokio::sync::{oneshot, Barrier};
use tokio::time::{timeout, Duration};

fn key(owner_type: &str, owner_id: &str, topic_id: &str, msg_id: &str) -> MessageKey {
    message_key_from_parts(owner_id, owner_type, topic_id, msg_id).unwrap()
}

#[tokio::test]
async fn active_requests_keep_same_message_id_isolated_by_composite_key() {
    let registry = ActiveRequestRegistry::default();
    let agent_key = key("agent", "owner-a", "shared-topic", "same-msg");
    let group_key = key("group", "owner-g", "shared-topic", "same-msg");
    let (agent_tx, _agent_rx) = oneshot::channel();
    let (group_tx, _group_rx) = oneshot::channel();

    registry.register(agent_key.clone(), agent_tx).await;
    registry.register(group_key.clone(), group_tx).await;

    assert!(registry.contains_key(&agent_key));
    assert!(registry.contains_key(&group_key));
    assert!(!registry.contains_key("same-msg"));
    assert!(registry.remove_key(&agent_key).is_some());
    assert!(!registry.contains_key(&agent_key));
    assert!(registry.contains_key(&group_key));
    assert!(registry.remove_key(&group_key).is_some());
    assert!(registry.is_empty());
}

#[tokio::test]
async fn legacy_remove_is_fail_closed_when_message_id_is_ambiguous() {
    let registry = ActiveRequestRegistry::default();
    registry
        .register(
            key("agent", "owner-a", "topic", "same-msg"),
            oneshot::channel().0,
        )
        .await;
    registry
        .register(
            key("group", "owner-g", "topic", "same-msg"),
            oneshot::channel().0,
        )
        .await;

    assert!(registry.remove("same-msg").is_none());
    assert_eq!(registry.len(), 2);
}

#[tokio::test]
async fn stale_epoch_cannot_remove_newer_registration() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, _old_lease) = registry.register(key.clone(), old_tx).await;
    let (new_tx, _new_rx) = oneshot::channel();
    let (new_epoch, old_sender, _new_lease) = registry.register(key.clone(), new_tx).await;
    assert!(old_sender.is_some());
    assert_ne!(old_epoch, new_epoch);
    assert!(!registry.remove_if_current(&key, old_epoch));
    assert!(registry.contains_key(&key));
    assert!(registry.remove_if_current(&key, new_epoch));
    assert!(registry.is_empty());
}

#[tokio::test]
async fn 旧请求终结不会影响新请求() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (_, _, old_lease) = registry.register(key.clone(), old_tx).await;
    let (new_tx, _new_rx) = oneshot::channel();
    let (_, _, new_lease) = registry.register(key, new_tx).await;

    let mut old_called = false;
    let old_result = old_lease
        .with_current_transition(|_| async {
            old_called = true;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(old_result, GuardedTransition::Skipped);
    assert!(!old_called);

    let new_result = new_lease
        .with_current_transition(|_| async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(new_result, GuardedTransition::Applied(()));
}

#[tokio::test]
async fn 完成租约释放后不再占用当前纪元() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (sender, _receiver) = oneshot::channel();
    let (epoch, _, lease) = registry.register(key.clone(), sender).await;
    assert!(registry.contains_key(&key));
    assert!(registry.remove_if_current(&key, epoch));
    drop(lease);
    assert!(!registry.contains_key(&key));
}

#[tokio::test]
async fn transport完成只移除取消sender并保留终结epoch() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "transport-topic", "same-msg");
    let (sender, _receiver) = oneshot::channel();
    let (epoch, _, lease) = registry.register(key.clone(), sender).await;

    assert!(registry.remove_entry_if_current(&key, epoch));
    assert!(registry.contains_key(&key));
    assert_eq!(
        lease
            .with_current_transition(|_| async { Ok::<_, String>(()) })
            .await
            .unwrap(),
        GuardedTransition::Applied(())
    );
    drop(lease);
    assert!(!registry.contains_key(&key));
}

#[tokio::test]
async fn context_requires_complete_owner_topic_message_identity() {
    let context = serde_json::json!({
        "ownerType": "group",
        "groupId": "owner-g",
        "agentId": "agent-a",
        "topicId": "shared-topic"
    });
    assert_eq!(
        message_key_from_context(Some(&context), "same-msg").unwrap(),
        key("group", "owner-g", "shared-topic", "same-msg")
    );
    assert!(message_key_from_context(None, "same-msg").is_err());
    assert!(
        message_key_from_context(Some(&serde_json::json!({"agentId": "owner-a"})), "same-msg")
            .is_err()
    );
}

#[test]
fn 上下文身份显式且冲突时安全失败() {
    let missing_type = serde_json::json!({
        "groupId": "owner-g",
        "topicId": "shared-topic"
    });
    assert!(message_key_from_context(Some(&missing_type), "same-msg").is_err());

    let mismatched_owner = serde_json::json!({
        "ownerType": "group",
        "ownerId": "owner-a",
        "groupId": "owner-g",
        "topicId": "shared-topic"
    });
    assert!(message_key_from_context(Some(&mismatched_owner), "same-msg").is_err());

    let speaker_is_not_owner = serde_json::json!({
        "ownerType": "group",
        "groupId": "owner-g",
        "agentId": "speaker-a",
        "topicId": "shared-topic",
        "messageId": "same-msg",
        "requestId": "same-msg"
    });
    assert_eq!(
        message_key_from_context(Some(&speaker_is_not_owner), "same-msg").unwrap(),
        key("group", "owner-g", "shared-topic", "same-msg")
    );

    let stale_payload_id = serde_json::json!({
        "ownerType": "agent",
        "agentId": "owner-a",
        "topicId": "shared-topic",
        "messageId": "old-msg"
    });
    assert!(message_key_from_context(Some(&stale_payload_id), "same-msg").is_err());
}

#[tokio::test]
async fn 同一转换锁下清理与申请不会分裂() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (a_sender, _a_receiver) = oneshot::channel();
    let (_, _, lease_a) = registry.register(key.clone(), a_sender).await;
    let barrier = Arc::new(Barrier::new(2));
    let (persist_tx, persist_rx) = oneshot::channel();
    let (release_a_tx, release_a_rx) = oneshot::channel();
    let (clear_tx, clear_rx) = oneshot::channel();
    let (end_tx, end_rx) = oneshot::channel();

    let a_barrier = barrier.clone();
    let finalize_a = tokio::spawn(async move {
        lease_a
            .with_current_transition(|_| async move {
                persist_tx.send(()).unwrap();
                a_barrier.wait().await;
                release_a_rx.await.unwrap();
                clear_tx.send(()).unwrap();
                end_tx.send(()).unwrap();
                Ok(())
            })
            .await
    });
    persist_rx.await.unwrap();

    let (b_started_tx, b_started_rx) = oneshot::channel();
    let (b_finished_tx, b_finished_rx) = oneshot::channel();
    let b_registry = registry.clone();
    let b_key = key.clone();
    let b_barrier = barrier.clone();
    let register_b = tokio::spawn(async move {
        b_started_tx.send(()).unwrap();
        b_barrier.wait().await;
        let result = b_registry.register(b_key, oneshot::channel().0).await;
        let _ = b_finished_tx.send(());
        result
    });
    b_started_rx.await.unwrap();
    assert!(timeout(Duration::from_millis(50), b_finished_rx)
        .await
        .is_err());

    release_a_tx.send(()).unwrap();
    assert_eq!(
        finalize_a.await.unwrap().unwrap(),
        GuardedTransition::Applied(())
    );
    clear_rx.await.unwrap();
    end_rx.await.unwrap();
    assert!(timeout(Duration::from_secs(1), register_b)
        .await
        .unwrap()
        .is_ok());
}

#[tokio::test]
async fn 最后强引用释放后同一消息重建转换锁() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let first = registry.transition_for(&key);
    let first_weak = Arc::downgrade(&first);
    let retained = first.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (done_tx, done_rx) = oneshot::channel();
    let release_task = tokio::spawn(async move {
        ready_tx.send(()).unwrap();
        release_rx.await.unwrap();
        drop(retained);
        done_tx.send(()).unwrap();
    });

    ready_rx.await.unwrap();
    let same = registry.transition_for(&key);
    assert!(Arc::ptr_eq(&first, &same));
    drop(same);
    release_tx.send(()).unwrap();
    done_rx.await.unwrap();
    drop(first);
    assert!(first_weak.upgrade().is_none());
    assert_eq!(registry.inner.transitions.len(), 0);
    release_task.await.unwrap();

    let (rebuilt_tx, rebuilt_rx) = oneshot::channel();
    let rebuild_registry = registry.clone();
    let rebuild_key = key.clone();
    let rebuild_task = tokio::spawn(async move {
        assert!(rebuilt_tx
            .send(rebuild_registry.transition_for(&rebuild_key))
            .is_ok());
    });
    let rebuilt = match rebuilt_rx.await {
        Ok(transition) => transition,
        Err(_) => panic!("重建转换锁任务未返回结果"),
    };
    assert_eq!(registry.inner.transitions.len(), 1);
    rebuild_task.await.unwrap();
    drop(rebuilt);
    assert_eq!(registry.inner.transitions.len(), 0);
}

#[tokio::test]
async fn claim_if_inactive在同一纪元下拒绝并发恢复() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let first = registry.claim_if_inactive(key.clone()).await.unwrap();
    let lease = match first {
        ClaimIfInactive::Claimed(lease) => lease,
        ClaimIfInactive::Active { .. } => panic!("首次恢复申请不应已占用"),
    };
    assert!(matches!(
        registry.claim_if_inactive(key).await.unwrap(),
        ClaimIfInactive::Active { .. }
    ));
    drop(lease);
    assert!(!registry.contains_key("same-msg"));
}

#[tokio::test]
async fn 恢复claim没有entry时仍可被话题编辑捕获并取消() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "recoverable-topic", "same-msg");
    let lease = match registry.claim_if_inactive(key.clone()).await.unwrap() {
        ClaimIfInactive::Claimed(lease) => lease,
        ClaimIfInactive::Active { .. } => panic!("首次恢复申请不应已占用"),
    };
    let snapshot = registry.snapshot_epochs_for_topic(&key.topic);
    assert_eq!(snapshot, vec![(key.clone(), lease.epoch())]);
    assert!(registry.cancel_if_current(&key, lease.epoch()).await);
    assert!(!registry.contains_key(&key));
    assert_eq!(
        lease
            .with_current_transition(|_| async { Ok::<_, String>(()) })
            .await
            .unwrap(),
        GuardedTransition::Skipped
    );
}

#[tokio::test]
async fn 活动请求返回独立的session_generation且旧绑定不能覆盖新请求() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));
    let active = registry.claim_if_inactive(key.clone()).await.unwrap();
    match active {
        ClaimIfInactive::Active { generation, .. } => assert_eq!(generation, Some(41)),
        ClaimIfInactive::Claimed(_) => panic!("活动请求不应被恢复流程再次占用"),
    }

    let (new_tx, _new_rx) = oneshot::channel();
    let (new_epoch, _, new_lease) = registry.register(key.clone(), new_tx).await;
    assert!(!registry.bind_session_generation(&key, old_epoch, 42));
    assert!(registry.bind_session_generation(&key, new_epoch, 43));
    match registry.claim_if_inactive(key).await.unwrap() {
        ClaimIfInactive::Active { generation, .. } => assert_eq!(generation, Some(43)),
        ClaimIfInactive::Claimed(_) => panic!("新请求不应被恢复流程再次占用"),
    }
    drop(old_lease);
    drop(new_lease);
}

#[tokio::test]
async fn 同helper_generation接管使旧stop只断socket() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));

    let (new_tx, _new_rx) = oneshot::channel();
    let (new_epoch, old_sender, new_lease) = registry
        .register_for_generation(key.clone(), new_tx, 41)
        .await
        .expect("接管 generation 必须注册成功");
    assert_ne!(new_epoch, old_epoch);
    assert!(old_sender.is_some());
    assert!(registry.is_same_helper_generation_takeover(&key, old_epoch, 41));

    let mut stop_called = false;
    let stop = registry
        .with_helper_stop_authorization(&key, old_epoch, 41, || async {
            stop_called = true;
            Ok::<_, String>(())
        })
        .await
        .expect("接管后的旧 stop 判定不应失败");
    assert_eq!(stop, GuardedTransition::Skipped);
    assert!(!stop_called);

    drop(old_lease);
    drop(new_lease);
}

#[tokio::test]
async fn helper_generation不匹配时拒绝接管且不终止旧请求() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, mut old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));

    let (stale_tx, _stale_rx) = oneshot::channel();
    let error = match registry
        .register_for_generation(key.clone(), stale_tx, 42)
        .await
    {
        Ok(_) => panic!("不同 helper generation 的 resume 必须拒绝接管"),
        Err(error) => error,
    };
    assert!(error.contains("helper generation 不匹配"));
    assert!(matches!(
        old_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    match registry.claim_if_inactive(key).await.unwrap() {
        ClaimIfInactive::Active { generation, epoch } => {
            assert_eq!(generation, Some(41));
            assert_eq!(epoch, old_epoch);
        }
        ClaimIfInactive::Claimed(_) => panic!("旧请求不得被 stale resume 替换"),
    }
    drop(old_lease);
}

#[tokio::test]
async fn helper_stop与同generation接管共享转换边界() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));

    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let stop_registry = registry.clone();
    let stop_key = key.clone();
    let stop = tokio::spawn(async move {
        stop_registry
            .with_helper_stop_authorization(&stop_key, old_epoch, 41, || async move {
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, String>(())
            })
            .await
    });
    entered_rx.await.unwrap();

    let replacement_registry = registry.clone();
    let replacement_key = key.clone();
    let mut replacement = tokio::spawn(async move {
        replacement_registry
            .register_for_generation(replacement_key, oneshot::channel().0, 41)
            .await
    });
    assert!(timeout(Duration::from_millis(50), &mut replacement)
        .await
        .is_err());

    release_tx.send(()).unwrap();
    assert_eq!(stop.await.unwrap().unwrap(), GuardedTransition::Applied(()));
    let (_, _, new_lease) = replacement.await.unwrap().expect("接管应在 stop 后成功");
    drop(old_lease);
    drop(new_lease);
}

#[tokio::test]
async fn 用户取消移除旧条目后仍允许stop_helper() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));
    assert!(registry.remove_key_guarded(&key).await.is_some());

    let mut stop_called = false;
    let stop = registry
        .with_helper_stop_authorization(&key, old_epoch, 41, || async {
            stop_called = true;
            Ok::<_, String>(())
        })
        .await
        .expect("用户取消后的 stop 判定不应失败");
    assert_eq!(stop, GuardedTransition::Applied(()));
    assert!(stop_called);
    drop(old_lease);
}

#[tokio::test]
async fn helper_generation绑定等待持久化时阻塞替换并拒绝旧epoch() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let binding_registry = registry.clone();
    let binding_key = key.clone();
    let binding = tokio::spawn(async move {
        binding_registry
            .bind_session_generation_with(&binding_key, old_epoch, 77, || async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, String>(())
            })
            .await
    });
    started_rx.await.unwrap();

    let register_registry = registry.clone();
    let register_key = key.clone();
    let mut replacement = tokio::spawn(async move {
        register_registry
            .register(register_key, oneshot::channel().0)
            .await
    });
    assert!(timeout(Duration::from_millis(50), &mut replacement)
        .await
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        binding.await.unwrap().unwrap(),
        GuardedTransition::Applied(())
    );

    let (new_epoch, _, new_lease) = replacement.await.unwrap();
    assert_ne!(new_epoch, old_epoch);
    assert!(!registry.bind_session_generation(&key, old_epoch, 78));
    assert!(matches!(
        registry.claim_if_inactive(key).await.unwrap(),
        ClaimIfInactive::Active {
            generation: None,
            ..
        }
    ));
    drop(old_lease);
    drop(new_lease);
}

#[tokio::test]
async fn helper查询期间replacement等待并保持旧结果边界() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 61));

    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let query_registry = registry.clone();
    let query_key = key.clone();
    let query = tokio::spawn(async move {
        query_registry
            .with_active_generation(&query_key, old_epoch, 61, || async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, String>("旧查询结果")
            })
            .await
    });
    started_rx.await.unwrap();

    let replacement_registry = registry.clone();
    let replacement_key = key.clone();
    let mut replacement = tokio::spawn(async move {
        replacement_registry
            .register(replacement_key, oneshot::channel().0)
            .await
    });
    assert!(timeout(Duration::from_millis(50), &mut replacement)
        .await
        .is_err());

    release_tx.send(()).unwrap();
    assert_eq!(
        query.await.unwrap().unwrap(),
        GuardedTransition::Applied("旧查询结果")
    );
    let (new_epoch, _, new_lease) = replacement.await.unwrap();
    assert_ne!(new_epoch, old_epoch);
    assert!(matches!(
        registry.claim_if_inactive(key).await.unwrap(),
        ClaimIfInactive::Active {
            generation: None,
            epoch
        } if epoch == new_epoch
    ));
    drop(old_lease);
    drop(new_lease);
}

#[tokio::test]
async fn 删除捕获旧纪元后替换请求不会被旧取消命中() {
    let registry = ActiveRequestRegistry::default();
    let key = key("agent", "owner-a", "topic", "same-msg");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert_eq!(
        registry.snapshot_epochs_for_topic(&key.topic),
        vec![(key.clone(), old_epoch)]
    );

    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let held_transition = tokio::spawn(async move {
        old_lease
            .with_current_transition(|_| async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, String>(())
            })
            .await
    });
    started_rx.await.unwrap();

    let (new_tx, new_rx) = oneshot::channel();
    let replacement_registry = registry.clone();
    let replacement_key = key.clone();
    let replacement =
        tokio::spawn(async move { replacement_registry.register(replacement_key, new_tx).await });
    let mut replacement = replacement;
    assert!(timeout(Duration::from_millis(50), &mut replacement)
        .await
        .is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        held_transition.await.unwrap().unwrap(),
        GuardedTransition::Applied(())
    );
    let (new_epoch, old_sender, new_lease) = replacement.await.unwrap();
    drop(old_sender);
    assert_ne!(new_epoch, old_epoch);
    assert!(!registry.cancel_if_current(&key, old_epoch).await);
    assert!(timeout(Duration::from_millis(50), new_rx).await.is_err());
    drop(new_lease);
}

#[tokio::test]
async fn 话题编辑屏障阻止未落库旧租约并允许新纪元通过() {
    let registry = ActiveRequestRegistry::default();
    let topic = super::TopicKey::new("agent", "owner-a", "edit-topic");
    let key = key("agent", "owner-a", "edit-topic", "same-message");
    let (old_tx, mut old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;

    let (mutation_started_tx, mutation_started_rx) = oneshot::channel();
    let (release_mutation_tx, release_mutation_rx) = oneshot::channel();
    let mutation_registry = registry.clone();
    let mutation_topic = topic.clone();
    let mutation = tokio::spawn(async move {
        let cancellation_registry = mutation_registry.clone();
        mutation_registry
            .with_topic_mutation(&mutation_topic, move |captured| async move {
                mutation_started_tx.send(captured.clone()).unwrap();
                release_mutation_rx.await.unwrap();
                for (key, epoch) in captured {
                    cancellation_registry.cancel_if_current(&key, epoch).await;
                }
                Ok::<_, String>(())
            })
            .await
    });
    let captured = mutation_started_rx.await.unwrap();
    assert_eq!(captured, vec![(key.clone(), old_epoch)]);

    let (old_persist_started_tx, old_persist_started_rx) = oneshot::channel();
    let old_persist = tokio::spawn(async move {
        old_lease
            .with_current_transition(|_| async move {
                old_persist_started_tx.send(()).unwrap();
                Ok::<_, String>(())
            })
            .await
    });
    assert!(timeout(Duration::from_millis(50), old_persist_started_rx)
        .await
        .is_err());

    let (new_tx, mut new_rx) = oneshot::channel();
    let registration_registry = registry.clone();
    let registration_key = key.clone();
    let mut new_registration = tokio::spawn(async move {
        registration_registry
            .register(registration_key, new_tx)
            .await
    });
    assert!(timeout(Duration::from_millis(50), &mut new_registration)
        .await
        .is_err());

    release_mutation_tx.send(()).unwrap();
    assert_eq!(mutation.await.unwrap().unwrap(), ());
    assert_eq!(
        old_persist.await.unwrap().unwrap(),
        GuardedTransition::Skipped
    );
    assert!(matches!(old_rx.try_recv(), Ok(())));

    let (new_epoch, previous_sender, new_lease) = new_registration.await.unwrap();
    assert!(previous_sender.is_none());
    assert_ne!(new_epoch, old_epoch);
    assert_eq!(
        new_lease
            .with_current_transition(|_| async { Ok::<_, String>(()) })
            .await
            .unwrap(),
        GuardedTransition::Applied(())
    );
    assert!(timeout(Duration::from_millis(50), &mut new_rx)
        .await
        .is_err());
    drop(new_lease);
}

#[tokio::test]
async fn 话题编辑屏障按完整owner隔离() {
    let registry = ActiveRequestRegistry::default();
    let edited_topic = super::TopicKey::new("agent", "owner-a", "shared-topic");
    let other_key = key("group", "owner-g", "shared-topic", "same-message");
    let (other_tx, _other_rx) = oneshot::channel();
    let (_, _, other_lease) = registry.register(other_key, other_tx).await;

    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let mutation_registry = registry.clone();
    let mutation = tokio::spawn(async move {
        mutation_registry
            .with_topic_mutation(&edited_topic, move |captured| async move {
                assert!(captured.is_empty());
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                Ok::<_, String>(())
            })
            .await
    });
    started_rx.await.unwrap();

    assert_eq!(
        other_lease
            .with_current_transition(|_| async { Ok::<_, String>(()) })
            .await
            .unwrap(),
        GuardedTransition::Applied(())
    );
    release_tx.send(()).unwrap();
    assert_eq!(mutation.await.unwrap().unwrap(), ());
    drop(other_lease);
}
