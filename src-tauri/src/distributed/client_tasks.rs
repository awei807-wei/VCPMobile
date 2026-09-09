use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{release_wake_lock_helper, scoped_wake_lock_tag};
use tauri::AppHandle;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio::time::{timeout, Instant};

const DERIVED_TASK_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const SESSION_STOP_TIMEOUT: Duration = Duration::from_secs(4);

pub(super) struct WakeLockLease {
    app: AppHandle,
    tag: String,
    generation: u64,
    session_generation: Arc<AtomicU64>,
}

impl WakeLockLease {
    pub(super) fn new(
        app: &AppHandle,
        tag: impl Into<String>,
        session_generation: &Arc<AtomicU64>,
        generation: u64,
    ) -> Self {
        Self {
            app: app.clone(),
            tag: scoped_wake_lock_tag(&tag.into(), generation),
            generation,
            session_generation: session_generation.clone(),
        }
    }
}

impl Drop for WakeLockLease {
    fn drop(&mut self) {
        if self.session_generation.load(Ordering::SeqCst) != self.generation {
            log::debug!(
                "[Distributed] 回收过期 generation={} 的保活 lease：{}",
                self.generation,
                self.tag
            );
        }
        release_wake_lock_helper(&self.app, &self.tag);
    }
}

/// 单个连接持有的派生任务集合，统一负责工具、IP 上报和占位符任务收口。
#[derive(Clone)]
pub(super) struct SessionTaskRegistry {
    tasks: Arc<Mutex<JoinSet<()>>>,
    closed: Arc<AtomicBool>,
}

impl SessionTaskRegistry {
    pub(super) fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(JoinSet::new())),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) async fn spawn<F>(&self, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        let mut tasks = self.tasks.lock().await;
        self.reap_finished_locked(&mut tasks);
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        tasks.spawn(task);
        true
    }

    pub(super) fn invalidate(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub(super) async fn abort_all(&self) {
        self.abort_and_join().await;
    }

    pub(super) async fn abort_and_join(&self) {
        self.invalidate();
        let mut tasks = self.tasks.lock().await;
        tasks.abort_all();
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::debug!("[Distributed] 已回收被中止的派生任务：{}", error);
            }
        }
    }

    pub(super) async fn shutdown(&self) {
        self.invalidate();
        let deadline = Instant::now() + DERIVED_TASK_DRAIN_TIMEOUT;
        let mut tasks = self.tasks.lock().await;
        while !tasks.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                tasks.abort_all();
                break;
            }
            match timeout(remaining, tasks.join_next()).await {
                Ok(Some(Ok(()))) => {}
                Ok(Some(Err(error))) => {
                    log::warn!("[Distributed] 派生任务异常退出：{}", error);
                }
                Ok(None) => break,
                Err(_) => {
                    log::warn!("[Distributed] 派生任务收口超时，执行中止。");
                    tasks.abort_all();
                    break;
                }
            }
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                log::debug!("[Distributed] 已回收被中止的派生任务：{}", error);
            }
        }
    }

    fn reap_finished_locked(&self, tasks: &mut JoinSet<()>) {
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                log::debug!("[Distributed] 已回收完成的派生任务：{}", error);
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.tasks.lock().await.len()
    }
}
