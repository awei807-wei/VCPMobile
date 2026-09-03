use crate::vcp_modules::group_types::GroupConfig;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// GroupManagerState 的全局状态。
pub struct GroupManagerState {
    /// 配置缓存: group_id -> GroupConfig
    pub caches: DashMap<String, GroupConfig>,
    /// 任务队列锁: group_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    pub async fn acquire_lock(&self, group_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(group_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
    }
}
