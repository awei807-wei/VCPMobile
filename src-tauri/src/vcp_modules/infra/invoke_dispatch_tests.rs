use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc as std_mpsc, Arc, Barrier as SyncBarrier,
    },
    time::Duration,
};

use serde_json::Value;
use tauri::{
    ipc::{CallbackFn, InvokeBody, InvokeHandler, InvokeResponse, InvokeResponseBody},
    test::MockRuntime,
    webview::InvokeRequest,
    Manager, State, WebviewWindow, WebviewWindowBuilder,
};
use tokio::sync::{mpsc, Barrier};

use super::{central_invoke_handler, central_invoke_handler_with_gate, BeforeDispatchGate};
use crate::vcp_modules::db_manager::{DbState, CORE_NOT_READY_ERROR};

const CONCURRENT_INVOCATIONS: usize = 64;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct BarrierCommandState {
    barrier: Arc<Barrier>,
    calls: Arc<AtomicUsize>,
}

impl BarrierCommandState {
    fn new(parties: usize) -> Self {
        Self {
            barrier: Arc::new(Barrier::new(parties)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// 最小真实异步命令：用于证明 central wrapper 不会在入口同步 poll handler。
#[tauri::command]
async fn ipc_dispatch_barrier(
    value: String,
    state: State<'_, BarrierCommandState>,
) -> Result<String, String> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    state.barrier.wait().await;
    Ok(value)
}

#[tauri::command]
async fn ipc_dispatch_echo(value: String) -> Result<String, String> {
    Ok(value)
}

struct ResponseRecord {
    command: String,
    response: InvokeResponse,
    callback: CallbackFn,
    error: CallbackFn,
}

fn test_handler() -> Arc<InvokeHandler<MockRuntime>> {
    Arc::new(tauri::generate_handler![
        ipc_dispatch_barrier,
        ipc_dispatch_echo
    ])
}

fn gated_sync_handler(
    calls: Arc<AtomicUsize>,
    release: Arc<SyncBarrier>,
) -> Arc<InvokeHandler<MockRuntime>> {
    Arc::new(move |invoke| {
        calls.fetch_add(1, Ordering::SeqCst);
        release.wait();
        invoke.resolver.resolve("sync-worker");
        true
    })
}

fn test_app_with_gate(
    commands: Arc<InvokeHandler<MockRuntime>>,
    before_dispatch_gate: Option<BeforeDispatchGate>,
) -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .invoke_handler(central_invoke_handler_with_gate(
            commands,
            before_dispatch_gate,
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("创建 MockRuntime 应用失败")
}

fn test_app() -> tauri::App<MockRuntime> {
    tauri::test::mock_builder()
        .invoke_handler(central_invoke_handler(test_handler()))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("创建 MockRuntime 应用失败")
}

fn worker_gate() -> (BeforeDispatchGate, Arc<SyncBarrier>, Arc<SyncBarrier>) {
    let worker_reached = Arc::new(SyncBarrier::new(2));
    let worker_release = Arc::new(SyncBarrier::new(2));
    let gate_reached = worker_reached.clone();
    let gate_release = worker_release.clone();
    let gate: BeforeDispatchGate = Arc::new(move || {
        gate_reached.wait();
        gate_release.wait();
    });
    (gate, worker_reached, worker_release)
}

fn test_window(app: &tauri::App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("创建 MockRuntime Webview 失败")
}

async fn test_db_state() -> DbState {
    DbState {
        pool: sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("创建测试数据库连接池失败"),
        path: PathBuf::from("invoke-dispatch-test.sqlite"),
    }
}

fn local_url() -> url::Url {
    let raw = if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    raw.parse().expect("测试 URL 无效")
}

fn submit_invoke(
    window: &WebviewWindow<MockRuntime>,
    command: &str,
    payload: Value,
    callback_id: u32,
    sender: &mpsc::UnboundedSender<ResponseRecord>,
) {
    let sender = sender.clone();
    window.clone().on_message(
        InvokeRequest {
            cmd: command.to_string(),
            callback: CallbackFn(callback_id),
            error: CallbackFn(callback_id + 10_000),
            url: local_url(),
            body: InvokeBody::Json(payload),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
        Box::new(move |_window, command, response, callback, error| {
            let _ = sender.send(ResponseRecord {
                command,
                response,
                callback,
                error,
            });
        }),
    );
}

async fn next_response(receiver: &mut mpsc::UnboundedReceiver<ResponseRecord>) -> ResponseRecord {
    tokio::time::timeout(RESPONSE_TIMEOUT, receiver.recv())
        .await
        .expect("IPC 响应超时")
        .expect("IPC 响应通道意外关闭")
}

async fn assert_no_response(receiver: &mut mpsc::UnboundedReceiver<ResponseRecord>) {
    assert!(
        tokio::time::timeout(Duration::from_millis(100), receiver.recv())
            .await
            .is_err(),
        "同一个 resolver 不应收到第二次响应"
    );
}

fn response_text(response: InvokeResponse) -> String {
    match response {
        InvokeResponse::Ok(InvokeResponseBody::Json(body)) => {
            serde_json::from_str(&body).expect("成功响应应为 JSON 字符串")
        }
        other => panic!("预期成功响应，实际为 {other:?}"),
    }
}

fn error_text(response: InvokeResponse) -> String {
    match response {
        InvokeResponse::Err(error) => match error.0 {
            Value::String(message) => message,
            value => value.to_string(),
        },
        other => panic!("预期错误响应，实际为 {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn central_dispatch_handles_success_argument_error_and_unknown_once() {
    let app = test_app();
    app.manage(test_db_state().await);
    let window = test_window(&app);
    let (sender, mut receiver) = mpsc::unbounded_channel();

    submit_invoke(
        &window,
        "ipc_dispatch_echo",
        serde_json::json!({"value": "worker"}),
        1,
        &sender,
    );
    let success = next_response(&mut receiver).await;
    assert_eq!(success.command, "ipc_dispatch_echo");
    assert_eq!(success.callback, CallbackFn(1));
    assert_eq!(success.error, CallbackFn(10_001));
    assert_eq!(response_text(success.response), "worker");
    assert_no_response(&mut receiver).await;

    submit_invoke(
        &window,
        "ipc_dispatch_echo",
        serde_json::json!({"value": 7}),
        2,
        &sender,
    );
    let argument_error = next_response(&mut receiver).await;
    assert_eq!(argument_error.command, "ipc_dispatch_echo");
    assert_eq!(argument_error.callback, CallbackFn(2));
    assert!(error_text(argument_error.response).contains("ipc_dispatch_echo"));
    assert_no_response(&mut receiver).await;

    submit_invoke(
        &window,
        "ipc_dispatch_unknown",
        serde_json::json!({}),
        3,
        &sender,
    );
    let unknown = next_response(&mut receiver).await;
    assert_eq!(unknown.command, "ipc_dispatch_unknown");
    assert_eq!(unknown.callback, CallbackFn(3));
    assert_eq!(
        error_text(unknown.response),
        "Command ipc_dispatch_unknown not found"
    );
    assert_no_response(&mut receiver).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn central_dispatch_snapshots_core_not_ready_before_worker_registration() {
    let db_state = test_db_state().await;
    let (gate, worker_reached, worker_release) = worker_gate();
    let app = test_app_with_gate(test_handler(), Some(gate));
    let state = BarrierCommandState::new(1);
    app.manage(state.clone());
    let window = test_window(&app);
    let (sender, mut receiver) = mpsc::unbounded_channel();

    submit_invoke(
        &window,
        "ipc_dispatch_barrier",
        serde_json::json!({"value": "must-reject"}),
        4,
        &sender,
    );
    worker_reached.wait();
    // DbState 预先创建且在 worker gate 期间同步注册，避免用 await 制造竞态。
    assert!(app.manage(db_state), "测试应在到达后首次注册 DbState");
    worker_release.wait();

    let rejected = next_response(&mut receiver).await;
    assert_eq!(rejected.command, "ipc_dispatch_barrier");
    assert!(error_text(rejected.response).starts_with(CORE_NOT_READY_ERROR));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    assert_no_response(&mut receiver).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn central_dispatch_runs_sync_handler_after_entry_returns() {
    let (gate, worker_reached, worker_release) = worker_gate();
    let handler_release = Arc::new(SyncBarrier::new(2));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = test_app_with_gate(
        gated_sync_handler(calls.clone(), handler_release.clone()),
        Some(gate),
    );
    app.manage(test_db_state().await);
    let window = test_window(&app);
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let (entry_sender, entry_receiver) = std_mpsc::sync_channel(1);
    let submit_window = window.clone();
    let submit_sender = sender.clone();
    let submit = std::thread::spawn(move || {
        submit_invoke(
            &submit_window,
            "ipc_dispatch_sync_fake",
            serde_json::json!({}),
            5,
            &submit_sender,
        );
        entry_sender.send(()).expect("入口完成信号接收端已关闭");
    });

    // 先观察入口线程已返回，再放行 worker；同步假 handler 本身不能伪造这个顺序。
    let entry_returned = entry_receiver.recv_timeout(RESPONSE_TIMEOUT).is_ok();
    if entry_returned {
        worker_reached.wait();
        worker_release.wait();
    }
    handler_release.wait();
    submit.join().expect("同步假 handler 提交线程失败");

    assert!(
        entry_returned,
        "commands(invoke) 必须在 central 入口返回后才执行"
    );
    let response = next_response(&mut receiver).await;
    assert_eq!(response.command, "ipc_dispatch_sync_fake");
    assert_eq!(response.callback, CallbackFn(5));
    assert_eq!(response_text(response.response), "sync-worker");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_no_response(&mut receiver).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn central_dispatch_async_barrier_resolves_64_invocations_exactly_once() {
    let app = test_app();
    app.manage(test_db_state().await);
    let state = BarrierCommandState::new(CONCURRENT_INVOCATIONS);
    app.manage(state.clone());
    let window = test_window(&app);
    let (sender, mut receiver) = mpsc::unbounded_channel();

    for index in 0..CONCURRENT_INVOCATIONS {
        submit_invoke(
            &window,
            "ipc_dispatch_barrier",
            serde_json::json!({"value": format!("value-{index}")}),
            100 + index as u32,
            &sender,
        );
    }

    let mut callbacks = HashSet::new();
    let mut values = HashSet::new();
    for _ in 0..CONCURRENT_INVOCATIONS {
        let response = next_response(&mut receiver).await;
        assert_eq!(response.command, "ipc_dispatch_barrier");
        assert!(callbacks.insert(response.callback.0), "callback 被重复消费");
        assert!(values.insert(response_text(response.response)));
    }
    assert_eq!(callbacks.len(), CONCURRENT_INVOCATIONS);
    assert_eq!(values.len(), CONCURRENT_INVOCATIONS);
    assert_eq!(state.calls.load(Ordering::SeqCst), CONCURRENT_INVOCATIONS);
    assert_no_response(&mut receiver).await;
}
