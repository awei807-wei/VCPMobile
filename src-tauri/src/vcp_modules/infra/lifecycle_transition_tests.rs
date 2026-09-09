use super::{resolve_foreground_epoch, should_reconnect_log_connections, LifecycleState};
use crate::vcp_modules::settings_manager::Settings;
use std::sync::atomic::Ordering;

#[test]
fn 隐式前后台请求不会把自身当作重复请求() {
    let state = LifecycleState::new();

    let background_epoch =
        resolve_foreground_epoch(&state, None, false).expect("后台请求应分配新序号");
    state
        .foreground_request_epoch
        .store(background_epoch, Ordering::SeqCst);
    state.is_foreground.store(false, Ordering::SeqCst);

    let foreground_epoch =
        resolve_foreground_epoch(&state, None, true).expect("前台请求应分配新序号");
    assert!(foreground_epoch > background_epoch);
    assert_eq!(
        state.foreground_request_epoch.load(Ordering::SeqCst),
        foreground_epoch
    );
}

#[test]
fn 前台后台前台顺序接受连续隐式请求() {
    let state = LifecycleState::new();
    let mut expected_epoch = 0;

    for is_foreground in [false, true, false, true] {
        let epoch =
            resolve_foreground_epoch(&state, None, is_foreground).expect("连续隐式请求应接受");
        assert!(epoch > expected_epoch);
        expected_epoch = epoch;
        state
            .foreground_request_epoch
            .store(epoch, Ordering::SeqCst);
        state.is_foreground.store(is_foreground, Ordering::SeqCst);
    }
}

#[test]
fn 禁用分布式不影响前台日志重连决策() {
    let settings = Settings {
        distributed_enabled: false,
        vcp_log_url: "ws://127.0.0.1:5800".to_string(),
        vcp_log_key: "log-key".to_string(),
        ..Settings::default()
    };

    assert!(should_reconnect_log_connections(&settings, true));
}

#[test]
fn 未发生日志冷断开时只冲刷后台日志() {
    let settings = Settings {
        vcp_log_url: "ws://127.0.0.1:5800".to_string(),
        vcp_log_key: "log-key".to_string(),
        ..Settings::default()
    };

    assert!(!should_reconnect_log_connections(&settings, false));
}
