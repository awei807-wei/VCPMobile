use crate::vcp_modules::agent_types::AgentConfig;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// AgentConfigState 的全局状态。
pub struct AgentConfigState {
    /// 配置缓存: agent_id -> AgentConfig
    pub caches: DashMap<String, AgentConfig>,
    /// 任务队列锁: agent_id -> Mutex
    pub locks: DashMap<String, Arc<Mutex<()>>>,
}

impl AgentConfigState {
    pub fn new() -> Self {
        Self {
            caches: DashMap::new(),
            locks: DashMap::new(),
        }
    }

    pub async fn acquire_lock(&self, agent_id: &str) -> Arc<Mutex<()>> {
        self.locks
            .entry(agent_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .value()
            .clone()
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
