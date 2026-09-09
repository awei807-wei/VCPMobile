use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

struct TestLogDir(std::path::PathBuf);

impl TestLogDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vcp-mobile-sync-logger-{}-{}",
            std::process::id(),
            NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).expect("create test log directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestLogDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parses_all_configured_log_levels_case_insensitively() {
    assert_eq!(LogLevel::parse("trace"), Some(LogLevel::Trace));
    assert_eq!(LogLevel::parse("DEBUG"), Some(LogLevel::Debug));
    assert_eq!(LogLevel::parse(" info "), Some(LogLevel::Info));
    assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warning));
    assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
    assert_eq!(LogLevel::parse("verbose"), None);
}

#[test]
fn redacts_secrets_but_keeps_diagnostic_locations_and_ids() {
    let input = concat!(
        "Authorization: Bearer top-secret; ",
        "syncToken=alpha, X-Sync-Token: beta, ",
        "ws://192.168.1.9:5890/sync?token=gamma&sync%5Ftoken=delta&topic=topic-7 ",
        "/data/user/0/com.vcp.avatar/logs/session.log"
    );
    let output = redact_sync_diagnostic(input);

    assert!(!output.contains("top-secret"));
    assert!(!output.contains("alpha"));
    assert!(!output.contains("beta"));
    assert!(!output.contains("gamma"));
    assert!(!output.contains("delta"));
    assert!(output.contains("192.168.1.9:5890"));
    assert!(output.contains("topic-7"));
    assert!(output.contains("/data/user/0/com.vcp.avatar/logs/session.log"));
}

#[test]
fn creates_a_unique_session_file_with_standard_levels() {
    let dir = TestLogDir::new();
    let mut logger = SyncLogger::new_session(LogLevel::Trace, Some(dir.path().into()), 42);
    logger.log(LogLevel::Warning, "network", "retrying");
    logger.end_session();

    let path = logger.log_path().expect("log path");
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("_42_sync.log"));
    let content = std::fs::read_to_string(path).expect("read log");
    assert!(content.contains("[INFO] [session] Session started (session_id=42)"));
    assert!(content.contains("[WARN] [network] retrying"));
}

#[test]
fn reports_log_creation_failure_without_preventing_logger_use() {
    let dir = TestLogDir::new();
    let blocked_path = dir.path().join("not-a-directory");
    std::fs::write(&blocked_path, b"occupied").expect("create blocking file");

    let mut logger = SyncLogger::new_session(LogLevel::Info, Some(blocked_path), 9);
    assert!(logger.log_path().is_none());
    assert!(logger.initialization_error().is_some());
    logger.log(LogLevel::Info, "sync", "continues without a file");
}
