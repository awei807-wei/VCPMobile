use super::{
    bind_generation_with_timeout, bind_generation_with_timeout_or_cancel,
    stop_helper_generation_with_transport, verify_generation_ack_for_key, BindGenerationAttempt,
    HelperStopRequest,
};
use crate::vcp_modules::chat::topic_types::{MessageKey, TopicKey};
use crate::vcp_modules::infra::vcp_client::registry::ClaimIfInactive;
use crate::vcp_modules::infra::vcp_client::ActiveRequestRegistry;
use futures_util::future;
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn bind_error_runs_authorized_stop_once_with_complete_identity() {
    let registry = Arc::new(ActiveRequestRegistry::default());
    let key = MessageKey::new(TopicKey::new("agent", "owner-a", "topic-a"), "message-a");
    let (cancel_tx, _cancel_rx) = oneshot::channel();
    let (epoch, _, lease) = registry.register(key.clone(), cancel_tx).await;
    let stop_generation = Arc::new(AtomicU64::new(0));
    let stop_requests = Arc::new(Mutex::new(Vec::<HelperStopRequest>::new()));
    let binding_registry = registry.clone();
    let binding_key = key.clone();
    let stop_registry = registry.clone();
    let stop_key = key.clone();
    let stop_generation_for_bind = stop_generation.clone();
    let stop_requests_for_bind = stop_requests.clone();
    let ack = serde_json::json!({
        "requestId": "message-a",
        "messageId": "message-a",
        "ownerType": "agent",
        "ownerId": "owner-a",
        "topicId": "topic-a",
        "eventType": "started",
        "generation": 41
    });
    assert!(verify_generation_ack_for_key(&key, &ack).is_ok());

    let result = bind_generation_with_timeout(
        Duration::from_millis(10),
        async move {
            let transition = binding_registry
                .bind_session_generation_with(&binding_key, epoch, 41, || async {
                    Ok::<_, String>(())
                })
                .await?;
            assert!(matches!(
                transition,
                crate::vcp_modules::infra::vcp_client::registry::GuardedTransition::Applied(())
            ));
            Err::<(), _>("持久化绑定失败".to_string())
        },
        move || {
            let stop_registry = stop_registry.clone();
            let stop_key = stop_key.clone();
            let stop_generation = stop_generation_for_bind.clone();
            let stop_requests = stop_requests_for_bind.clone();
            async move {
                stop_helper_generation_with_transport(
                    stop_registry,
                    "message-a".to_string(),
                    stop_key,
                    epoch,
                    stop_generation,
                    41,
                    move |request| {
                        stop_requests.lock().unwrap().push(request);
                        async { Ok::<_, String>(()) }
                    },
                )
                .await;
            }
        },
    )
    .await;

    assert_eq!(result, Err("持久化绑定失败".to_string()));
    let requests = stop_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].message_id, "message-a");
    assert_eq!(requests[0].request_key, key);
    assert_eq!(requests[0].generation, 41);
    drop(requests);
    assert_eq!(stop_generation.load(Ordering::SeqCst), 41);
    assert!(registry.contains_key(&key));
    assert!(matches!(
        registry.claim_if_inactive(key.clone()).await.unwrap(),
        ClaimIfInactive::Active {
            generation: Some(41),
            epoch: active_epoch,
        } if active_epoch == epoch
    ));

    assert!(registry.remove_entry_if_current(
        &MessageKey::new(TopicKey::new("agent", "owner-a", "topic-a"), "message-a"),
        epoch,
    ));
    drop(lease);
    assert!(registry.is_empty());
}

#[tokio::test]
async fn bind_timeout_runs_authorized_stop_once_and_keeps_active_request() {
    let registry = Arc::new(ActiveRequestRegistry::default());
    let key = MessageKey::new(
        TopicKey::new("agent", "owner-a", "topic-timeout"),
        "message-t",
    );
    let (cancel_tx, _cancel_rx) = oneshot::channel();
    let (epoch, _, lease) = registry.register(key.clone(), cancel_tx).await;
    assert!(registry.bind_session_generation(&key, epoch, 77));
    let stop_generation = Arc::new(AtomicU64::new(0));
    let stop_requests = Arc::new(Mutex::new(Vec::<HelperStopRequest>::new()));
    let stop_registry = registry.clone();
    let stop_key = key.clone();
    let stop_generation_for_bind = stop_generation.clone();
    let stop_requests_for_bind = stop_requests.clone();

    let result = bind_generation_with_timeout(
        Duration::from_millis(10),
        future::pending::<Result<(), String>>(),
        move || {
            let stop_registry = stop_registry.clone();
            let stop_key = stop_key.clone();
            let stop_generation = stop_generation_for_bind.clone();
            let stop_requests = stop_requests_for_bind.clone();
            async move {
                stop_helper_generation_with_transport(
                    stop_registry,
                    "message-t".to_string(),
                    stop_key,
                    epoch,
                    stop_generation,
                    77,
                    move |request| {
                        stop_requests.lock().unwrap().push(request);
                        async { Ok::<_, String>(()) }
                    },
                )
                .await;
            }
        },
    )
    .await;

    assert_eq!(result, Err("绑定 helper generation 超时".to_string()));
    assert_eq!(stop_requests.lock().unwrap().len(), 1);
    assert_eq!(stop_generation.load(Ordering::SeqCst), 77);
    assert!(registry.contains_key(&key));
    drop(lease);
}

#[tokio::test]
async fn caller_cancellation_runs_same_authorized_stop_path() {
    let registry = Arc::new(ActiveRequestRegistry::default());
    let key = MessageKey::new(TopicKey::new("group", "owner-c", "topic-c"), "message-c");
    let (cancel_tx, _cancel_rx) = oneshot::channel();
    let (epoch, _, lease) = registry.register(key.clone(), cancel_tx).await;
    assert!(registry.bind_session_generation(&key, epoch, 91));
    let (abort_tx, mut abort_rx) = oneshot::channel();
    abort_tx.send(()).unwrap();
    let stop_generation = Arc::new(AtomicU64::new(0));
    let stop_requests = Arc::new(Mutex::new(Vec::<HelperStopRequest>::new()));
    let stop_registry = registry.clone();
    let stop_key = key.clone();
    let stop_generation_for_bind = stop_generation.clone();
    let stop_requests_for_bind = stop_requests.clone();

    let attempt = bind_generation_with_timeout_or_cancel(
        Duration::from_secs(5),
        &mut abort_rx,
        future::pending::<Result<(), String>>(),
        move || {
            let stop_registry = stop_registry.clone();
            let stop_key = stop_key.clone();
            let stop_generation = stop_generation_for_bind.clone();
            let stop_requests = stop_requests_for_bind.clone();
            async move {
                stop_helper_generation_with_transport(
                    stop_registry,
                    "message-c".to_string(),
                    stop_key,
                    epoch,
                    stop_generation,
                    91,
                    move |request| {
                        stop_requests.lock().unwrap().push(request);
                        async { Ok::<_, String>(()) }
                    },
                )
                .await;
            }
        },
    )
    .await;

    assert_eq!(attempt, BindGenerationAttempt::Cancelled);
    assert_eq!(stop_requests.lock().unwrap().len(), 1);

    let stop_requests_for_second_stop = stop_requests.clone();
    stop_helper_generation_with_transport(
        registry.clone(),
        "message-c".to_string(),
        key.clone(),
        epoch,
        stop_generation,
        91,
        move |request| {
            stop_requests_for_second_stop.lock().unwrap().push(request);
            async { Ok::<_, String>(()) }
        },
    )
    .await;
    assert_eq!(stop_requests.lock().unwrap().len(), 1);
    drop(lease);
}

#[tokio::test]
async fn same_generation_takeover_skips_old_stop_and_different_generation_cannot_stop_new_one() {
    let registry = Arc::new(ActiveRequestRegistry::default());
    let key = MessageKey::new(TopicKey::new("agent", "owner-r", "topic-r"), "message-r");
    let (old_tx, _old_rx) = oneshot::channel();
    let (old_epoch, _, old_lease) = registry.register(key.clone(), old_tx).await;
    assert!(registry.bind_session_generation(&key, old_epoch, 41));

    let (_new_epoch, _old_sender, new_lease) = registry
        .register_for_generation(key.clone(), oneshot::channel().0, 41)
        .await
        .expect("同 generation 接管应成功");
    let same_generation_calls = Arc::new(AtomicUsize::new(0));
    stop_helper_generation_with_transport(
        registry.clone(),
        "message-r".to_string(),
        key.clone(),
        old_epoch,
        Arc::new(AtomicU64::new(0)),
        41,
        {
            let same_generation_calls = same_generation_calls.clone();
            move |_request| {
                same_generation_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, String>(()) }
            }
        },
    )
    .await;
    assert_eq!(same_generation_calls.load(Ordering::SeqCst), 0);

    let (next_tx, _next_rx) = oneshot::channel();
    let (next_epoch, _, next_lease) = registry.register(key.clone(), next_tx).await;
    assert!(registry.bind_session_generation(&key, next_epoch, 42));
    let stop_requests = Arc::new(Mutex::new(Vec::<HelperStopRequest>::new()));
    let actual_helper_stops = Arc::new(AtomicUsize::new(0));
    let stop_requests_for_transport = stop_requests.clone();
    let actual_helper_stops_for_transport = actual_helper_stops.clone();
    stop_helper_generation_with_transport(
        registry.clone(),
        "message-r".to_string(),
        key.clone(),
        old_epoch,
        Arc::new(AtomicU64::new(0)),
        41,
        move |request| {
            let stop_requests = stop_requests_for_transport.clone();
            let actual_helper_stops = actual_helper_stops_for_transport.clone();
            async move {
                if request.generation == 42 {
                    actual_helper_stops.fetch_add(1, Ordering::SeqCst);
                }
                stop_requests.lock().unwrap().push(request);
                Ok(())
            }
        },
    )
    .await;
    assert_eq!(actual_helper_stops.load(Ordering::SeqCst), 0);
    assert_eq!(stop_requests.lock().unwrap()[0].generation, 41);
    assert_eq!(stop_requests.lock().unwrap()[0].request_key, key);

    drop(old_lease);
    drop(new_lease);
    drop(next_lease);
}
