use super::{
    is_session_current, scoped_wake_lock_tag, ConnectionSession, DistributedClient,
    SessionTaskRegistry,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{oneshot, Barrier};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn stop声明在取得转换锁前立即失效旧session及派生任务() {
    let client = DistributedClient::new();
    let start_request = client.reserve_start_request();
    let session_id = client.next_session_generation();
    let cancel_token = CancellationToken::new();
    let task_registry = SessionTaskRegistry::new();
    let (re_register_tx, _re_register_rx) = tokio::sync::mpsc::channel(1);
    let (reconnect_tx, _reconnect_rx) = tokio::sync::mpsc::channel(1);
    let task_handle = tokio::spawn(std::future::pending::<()>());
    *client.lock_session() = Some(ConnectionSession {
        session_id,
        cancel_token: cancel_token.clone(),
        re_register_tx,
        reconnect_tx,
        task_registry: task_registry.clone(),
        task_handle,
    });
    assert!(client.start_request_is_current(start_request));

    let transition_guard = client.transition.lock().await;
    let stop_request = client.reserve_stop_request();

    assert!(cancel_token.is_cancelled());
    assert!(!is_session_current(
        &client.session_generation,
        session_id,
        &cancel_token
    ));
    assert!(!task_registry.spawn(async {}).await);

    drop(stop_request);
    drop(transition_guard);
    let old_session = client.lock_session().take().expect("旧 session 应可收口");
    old_session.task_handle.abort();
    let _ = old_session.task_handle.await;
}

#[test]
fn 不同session_generation使用不同保活所有者标签() {
    assert_ne!(
        scoped_wake_lock_tag("distributed", 1),
        scoped_wake_lock_tag("distributed", 2)
    );
}

#[test]
#[allow(non_snake_case)]
fn 分布式保活tag与Android共享主连接工具及旧占位样例() {
    let samples = [
        ("distributed", "distributed:generation:7"),
        (
            "distributed:connect:session:7",
            "distributed:connect:session:7:generation:7",
        ),
        (
            "distributed:connection:session:7",
            "distributed:connection:session:7:generation:7",
        ),
        (
            "distributed:tool:req-7:session:7",
            "distributed:tool:req-7:session:7:generation:7",
        ),
        (
            "distributed:placeholder_push:session:7",
            "distributed:placeholder_push:session:7:generation:7",
        ),
    ];

    for (base_tag, expected) in samples {
        assert_eq!(scoped_wake_lock_tag(base_tag, 7), expected);
    }
}

#[tokio::test]
async fn 启动遇到停止屏障时不会安装旧请求() {
    let client = DistributedClient::new();
    let old_start = client.reserve_start_request();
    let stop_request = client.reserve_stop_request();

    assert!(!client.start_request_is_current(old_start));
    assert_eq!(client.stop_in_flight.load(Ordering::SeqCst), 1);

    let stop_id = stop_request.request_id();
    drop(stop_request);
    let next_start = client.reserve_start_request();
    assert!(next_start > stop_id);
    assert!(client.start_request_is_current(next_start));
}

#[tokio::test]
async fn 设置禁用和网络恢复按最新请求决定结果() {
    let client = DistributedClient::new();
    let network_restore = client.reserve_start_request();
    let disable = client.reserve_stop_request();
    let foreground_restore = client.reserve_start_request();

    assert!(!client.request_is_latest(network_restore));
    assert!(!client.start_request_is_current(network_restore));
    assert!(!client.request_is_latest(disable.request_id()));
    assert!(!client.start_request_is_current(foreground_restore));

    drop(disable);
    assert_eq!(client.stop_in_flight.load(Ordering::SeqCst), 0);
    assert!(!client.start_request_is_current(foreground_restore));
    let after_stop = client.reserve_start_request();
    assert!(client.start_request_is_current(after_stop));
}

#[tokio::test]
async fn 活动停止屏障永久拒绝其期间预留的启动() {
    let client = DistributedClient::new();
    let _initial_start = client.reserve_start_request();
    let first_stop = client.reserve_stop_request();
    let blocked_start = client.reserve_start_request();
    let second_stop = client.reserve_stop_request();

    assert!(!client.start_request_is_current(blocked_start));
    assert!(client.stop_epoch.load(Ordering::SeqCst) >= second_stop.request_id());
    drop(first_stop);
    assert_eq!(client.stop_in_flight.load(Ordering::SeqCst), 1);
    assert!(!client.start_request_is_current(blocked_start));
    drop(second_stop);
    assert_eq!(client.stop_in_flight.load(Ordering::SeqCst), 0);
    assert!(!client.start_request_is_current(blocked_start));
    let fresh_start = client.reserve_start_request();
    assert!(client.start_request_is_current(fresh_start));
}

#[tokio::test]
async fn 并发停止预留保持序号单调且不穿透屏障() {
    let client = Arc::new(DistributedClient::new());
    let barrier = Arc::new(Barrier::new(3));
    let first_client = client.clone();
    let first_barrier = barrier.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_client.reserve_stop_request()
    });
    let second_client = client.clone();
    let second_barrier = barrier.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_client.reserve_stop_request()
    });
    barrier.wait().await;
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert_ne!(first.request_id(), second.request_id());
    assert_eq!(client.stop_in_flight.load(Ordering::SeqCst), 2);

    let blocked_start = client.reserve_start_request();
    assert!(!client.start_request_is_current(blocked_start));
    drop(first);
    drop(second);
    let fresh_start = client.reserve_start_request();
    assert!(client.start_request_is_current(fresh_start));
}

#[tokio::test]
async fn 中止派生任务后等待其退出() {
    let registry = SessionTaskRegistry::new();
    let (entered_tx, entered_rx) = oneshot::channel();
    assert!(
        registry
            .spawn(async move {
                entered_tx.send(()).unwrap();
                std::future::pending::<()>().await;
            })
            .await
    );
    entered_rx.await.unwrap();
    registry.abort_and_join().await;
    assert_eq!(registry.len().await, 0);
    assert!(!registry.spawn(async {}).await);
}

#[tokio::test]
async fn 新派生任务启动前会回收已完成任务() {
    let registry = SessionTaskRegistry::new();
    let (done_tx, done_rx) = oneshot::channel();
    assert!(
        registry
            .spawn(async move {
                done_tx.send(()).unwrap();
            })
            .await
    );
    done_rx.await.unwrap();
    tokio::task::yield_now().await;
    assert!(registry.spawn(async {}).await);
    assert_eq!(registry.len().await, 1);
    registry.abort_and_join().await;
}

#[tokio::test]
async fn 启动停止网络恢复共享同一异步转换锁() {
    let client = Arc::new(DistributedClient::new());
    let entered = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = oneshot::channel();
    let first_guard = client.transition.lock().await;
    let waiting_client = client.clone();
    let waiting_entered = entered.clone();
    let waiting = tokio::spawn(async move {
        let _transition = waiting_client.transition.lock().await;
        waiting_entered.store(true, Ordering::SeqCst);
        let _ = release_rx.await;
    });

    tokio::task::yield_now().await;
    assert!(!entered.load(Ordering::SeqCst));
    drop(first_guard);
    let _ = release_tx.send(());
    waiting.await.expect("转换锁等待任务应正常结束");
    assert!(entered.load(Ordering::SeqCst));
}

#[test]
fn session_generation_stop后仍保持单调递增() {
    let client = DistributedClient::new();
    let first = client.next_session_generation();
    let second = client.next_session_generation();
    let after_stop = client.next_session_generation();

    assert!(first < second);
    assert!(second < after_stop);
}
