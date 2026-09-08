use crate::vcp_modules::agent_types::AgentConfig;
use crate::vcp_modules::owner_lock::{OwnerLockHandle, OwnerLockRegistry};
use std::sync::Arc;

/// Agent 配置服务状态只负责串行化实体写入。
///
/// 配置本身不在内存中缓存；读取始终从 SQLite 组装完整快照，避免同步、话题、未读
/// 或头像写入后还需要通知服务层失效缓存。锁注册表只为实际写入路径临时创建条目，
/// 最后一个持有者释放后自动回收。
pub struct AgentConfigState {
    owner_locks: Arc<OwnerLockRegistry>,
}

impl AgentConfigState {
    pub fn new() -> Self {
        Self::with_owner_locks(OwnerLockRegistry::new())
    }

    pub(crate) fn with_owner_locks(owner_locks: Arc<OwnerLockRegistry>) -> Self {
        Self { owner_locks }
    }

    pub(crate) async fn acquire_lock(&self, agent_id: &str) -> OwnerLockHandle {
        self.owner_locks.acquire_owner("agent", agent_id)
    }
}

pub fn create_default_config(agent_id: &str) -> AgentConfig {
    AgentConfig {
        id: agent_id.to_string(),
        name: "New Agent".to_string(),
        system_prompt: String::new(),
        mobile_system_prompt: String::new(),
        model: "gemini-2.5-flash".to_string(),
        temperature: 1.0,
        context_token_limit: 1_000_000,
        max_output_tokens: 64_000,
        stream_output: true,
        use_temperature: false,
        avatar_calculated_color: None,
        topics: vec![],
    }
}
