use crate::vcp_modules::owner_lock::{OwnerLockHandle, OwnerLockRegistry};
use std::sync::Arc;

/// Group 配置服务状态只负责串行化实体写入。
///
/// 配置不保存在内存缓存中；读取始终从 SQLite 事务快照组装完整配置。锁注册表仅在
/// 写入路径使用，并在最后一个持有者释放后回收，避免缺失 ID 请求造成无界增长。
pub struct GroupManagerState {
    owner_locks: Arc<OwnerLockRegistry>,
}

impl GroupManagerState {
    pub fn new() -> Self {
        Self::with_owner_locks(OwnerLockRegistry::new())
    }

    pub(crate) fn with_owner_locks(owner_locks: Arc<OwnerLockRegistry>) -> Self {
        Self { owner_locks }
    }

    pub(crate) async fn acquire_lock(&self, group_id: &str) -> OwnerLockHandle {
        self.owner_locks.acquire_owner("group", group_id)
    }
}
