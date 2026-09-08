use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tauri::AppHandle;
use tokio::sync::Mutex;

use super::types::ToolManifest;

const EXECUTION_TOOL_RENAME_ALIASES: &[(&str, &str)] = &[("TopicSponsor", "MobileTopicSponsor")];

#[path = "tool_registry_config.rs"]
mod config;
pub use config::ToolConfigStatus;
use config::{ConfigSource, ConfigState};

/// 一次性工具：执行后立即返回结果。
#[async_trait]
pub trait OneShotTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String>;
}

/// 交互工具：通过前端往返完成需要用户参与的动作。
#[allow(dead_code)]
#[async_trait]
pub trait InteractiveTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    async fn execute(&self, args: Value, app: &AppHandle) -> Result<Value, String>;
    fn required_permissions(&self) -> Vec<&'static str>;
}

/// 流式工具：提供静态占位符的当前快照。
pub trait StreamingTool: Send + Sync {
    fn manifest(&self) -> ToolManifest;
    fn placeholder_key(&self) -> &str;
    #[allow(dead_code)]
    fn poll_interval_secs(&self) -> u64;
    fn read_current(&self, app: &AppHandle) -> Result<String, String>;
}

#[allow(dead_code)]
pub enum ToolEntry {
    OneShot(Arc<dyn OneShotTool>),
    Interactive(Arc<dyn InteractiveTool>),
    Streaming(Arc<dyn StreamingTool>),
}

impl ToolEntry {
    pub fn manifest(&self) -> ToolManifest {
        match self {
            Self::OneShot(tool) => tool.manifest(),
            Self::Interactive(tool) => tool.manifest(),
            Self::Streaming(tool) => tool.manifest(),
        }
    }
}

enum ExecutableTool {
    OneShot(Arc<dyn OneShotTool>),
    Interactive(Arc<dyn InteractiveTool>),
    Streaming(Arc<dyn StreamingTool>),
}

/// 分布式工具注册表，策略采用显式 enabled allowlist。
pub struct ToolRegistry {
    tools: HashMap<String, ToolEntry>,
    enabled_config: Mutex<ConfigState>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            enabled_config: Mutex::new(ConfigState::default()),
        }
    }

    /// 在唯一配置锁下加载并校验 allowlist，未成功前保持拒绝策略。
    pub async fn ensure_enabled_config_loaded(&self, app: &AppHandle) -> Result<(), String> {
        let mut state = self.enabled_config.lock().await;
        self.ensure_enabled_config_loaded_locked(app, &mut state)
            .await
    }

    async fn ensure_enabled_config_loaded_locked(
        &self,
        app: &AppHandle,
        state: &mut ConfigState,
    ) -> Result<(), String> {
        if state.loaded {
            return Ok(());
        }
        let path = match config::config_path(app) {
            Ok(path) => path,
            Err(error) => return self.remember_load_error(state, error),
        };
        let content = match config::read_config(&path) {
            Ok(content) => content,
            Err(error) => return self.remember_load_error(state, error),
        };
        let (raw_names, source, needs_persist) = match content {
            None => (Vec::new(), ConfigSource::Default, true),
            Some(content) => match config::parse_config(&content) {
                Ok(config::ParsedConfig::Enabled(names)) => (names, ConfigSource::Explicit, false),
                Ok(config::ParsedConfig::LegacyDisabled) => {
                    log::warn!("[Distributed] 检测到旧版 disabled 配置，迁移为空 allowlist。");
                    (Vec::new(), ConfigSource::LegacyMigration, true)
                }
                Err(error) => return self.remember_load_error(state, error),
            },
        };
        let enabled_names = match self.validate_enabled_names(raw_names) {
            Ok(names) => names,
            Err(error) => return self.remember_load_error(state, error),
        };
        if needs_persist {
            if let Err(error) = config::persist_config(&path, &enabled_names) {
                return self.remember_load_error(state, error);
            }
        }
        state.enabled_names = enabled_names;
        state.source = source;
        state.loaded = true;
        state.last_error = None;
        log::info!(
            "[Distributed] 已加载工具 allowlist，启用 {} 个工具。",
            state.enabled_names.len()
        );
        Ok(())
    }

    fn remember_load_error<T>(&self, state: &mut ConfigState, error: String) -> Result<T, String> {
        state.last_error = Some(error.clone());
        log::error!("[Distributed] 工具 allowlist 加载失败，保持全部拒绝: {error}");
        Err(error)
    }

    fn validate_enabled_names(&self, names: Vec<String>) -> Result<HashSet<String>, String> {
        let mut enabled = HashSet::new();
        for name in names {
            let resolved = self
                .resolve_tool_name(&name)
                .ok_or_else(|| format!("工具 allowlist 包含未知工具，已拒绝: {name}"))?;
            if !enabled.insert(resolved.to_string()) {
                return Err(format!("工具 allowlist 包含重复工具: {name}"));
            }
        }
        Ok(enabled)
    }

    /// 更新 allowlist；只有原子落盘成功后才发布新的内存策略。
    pub async fn update_enabled(
        &self,
        app: &AppHandle,
        names: Vec<String>,
    ) -> Result<bool, String> {
        let mut state = self.enabled_config.lock().await;
        self.ensure_enabled_config_loaded_locked(app, &mut state)
            .await?;
        let new_names = self.validate_enabled_names(names)?;
        if new_names == state.enabled_names {
            return Ok(false);
        }
        let path = match config::config_path(app) {
            Ok(path) => path,
            Err(error) => return self.remember_persist_error(&mut state, error),
        };
        if let Err(error) = config::persist_config(&path, &new_names) {
            return self.remember_persist_error(&mut state, error);
        }
        state.enabled_names = new_names;
        state.source = ConfigSource::Explicit;
        state.last_error = None;
        Ok(true)
    }

    /// 安全重置为全拒绝 allowlist，供用户清除旧版 disabled 配置。
    pub async fn reset_enabled(&self, app: &AppHandle) -> Result<(), String> {
        let mut state = self.enabled_config.lock().await;
        self.ensure_enabled_config_loaded_locked(app, &mut state)
            .await?;
        if state.enabled_names.is_empty() {
            return Ok(());
        }
        let path = match config::config_path(app) {
            Ok(path) => path,
            Err(error) => return self.remember_persist_error(&mut state, error),
        };
        let empty = HashSet::new();
        if let Err(error) = config::persist_config(&path, &empty) {
            return self.remember_persist_error(&mut state, error);
        }
        state.enabled_names = empty;
        state.source = ConfigSource::Explicit;
        state.last_error = None;
        Ok(())
    }

    /// 返回 allowlist 状态；加载失败也通过 lastError 对外可见。
    pub async fn config_status(&self, app: &AppHandle) -> ToolConfigStatus {
        let _ = self.ensure_enabled_config_loaded(app).await;
        self.enabled_config.lock().await.status()
    }

    /// 同步兼容查询只读已成功加载的 allowlist，未加载时一律拒绝。
    pub fn is_enabled(&self, name: &str) -> bool {
        let resolved_name = self.resolve_tool_name(name).unwrap_or(name);
        self.enabled_config
            .try_lock()
            .map(|state| state.loaded && state.enabled_names.contains(resolved_name))
            .unwrap_or(false)
    }

    fn resolve_tool_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        if self.tools.contains_key(name) {
            return Some(name);
        }
        EXECUTION_TOOL_RENAME_ALIASES
            .iter()
            .find_map(|&(old_name, new_name)| {
                (name == old_name && self.tools.contains_key(new_name)).then_some(new_name)
            })
    }

    pub fn register_oneshot<T: OneShotTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools.insert(name, ToolEntry::OneShot(Arc::new(tool)));
    }

    #[allow(dead_code)]
    pub fn register_interactive<T: InteractiveTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools
            .insert(name, ToolEntry::Interactive(Arc::new(tool)));
    }

    pub fn register_streaming<T: StreamingTool + 'static>(&mut self, tool: T) {
        let name = tool.manifest().name.clone();
        self.tools
            .insert(name, ToolEntry::Streaming(Arc::new(tool)));
    }

    /// 获取远端注册用的启用工具集合。
    pub async fn get_all_manifests(&self, app: &AppHandle) -> Result<Vec<ToolManifest>, String> {
        let state = self.loaded_config(app).await?;
        Ok(self
            .tools
            .iter()
            .filter(|(name, _)| state.enabled_names.contains(*name))
            .map(|(_, entry)| entry.manifest())
            .collect())
    }

    /// 获取单个启用工具的 manifest。
    #[allow(dead_code)]
    pub async fn get_manifest(
        &self,
        app: &AppHandle,
        name: &str,
    ) -> Result<Option<ToolManifest>, String> {
        let state = self.loaded_config(app).await?;
        let Some(resolved_name) = self.resolve_tool_name(name) else {
            return Ok(None);
        };
        Ok(state
            .enabled_names
            .contains(resolved_name)
            .then(|| self.tools.get(resolved_name).map(ToolEntry::manifest))
            .flatten())
    }

    /// 获取前端配置页需要的工具 metadata。
    pub async fn get_tools_metadata(
        &self,
        app: &AppHandle,
    ) -> Result<Vec<serde_json::Value>, String> {
        let state = self.loaded_config(app).await?;
        Ok(self.build_tools_metadata(&state.enabled_names))
    }

    fn build_tools_metadata(&self, enabled_names: &HashSet<String>) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|(name, entry)| {
                let manifest = entry.manifest();
                let mut value = serde_json::to_value(&manifest).unwrap_or(serde_json::Value::Null);
                if let Some(object) = value.as_object_mut() {
                    let category = match entry {
                        ToolEntry::OneShot(_) => "oneshot",
                        ToolEntry::Interactive(_) => "interactive",
                        ToolEntry::Streaming(_) => "streaming",
                    };
                    object.insert("category".to_string(), serde_json::json!(category));
                    object.insert(
                        "enabled".to_string(),
                        serde_json::json!(enabled_names.contains(name)),
                    );
                    if let Some(placeholder) = &manifest.placeholder {
                        object.insert("placeholder".to_string(), serde_json::json!(placeholder));
                    }
                    object.insert(
                        "display_name".to_string(),
                        serde_json::json!(manifest.display_name),
                    );
                }
                value
            })
            .collect()
    }

    fn remember_persist_error<T>(
        &self,
        state: &mut ConfigState,
        error: String,
    ) -> Result<T, String> {
        state.last_error = Some(error.clone());
        log::error!("[Distributed] 工具 allowlist 持久化失败，未发布新状态: {error}");
        Err(error)
    }

    /// 获取已启用流式工具的当前占位符值。
    pub async fn get_all_placeholder_values(
        &self,
        app: &AppHandle,
    ) -> Result<HashMap<String, String>, String> {
        let state = self.loaded_config(app).await?;
        let mut values = HashMap::new();
        for (name, entry) in &self.tools {
            if state.enabled_names.contains(name) {
                if let ToolEntry::Streaming(tool) = entry {
                    if let Ok(value) = tool.read_current(app) {
                        values.insert(tool.placeholder_key().to_string(), value);
                    }
                }
            }
        }
        Ok(values)
    }

    /// 执行已启用工具，并在执行前确保 allowlist 已成功加载。
    pub async fn execute(
        &self,
        tool_name: &str,
        args: Value,
        app: &AppHandle,
    ) -> Result<Value, String> {
        let state = self.loaded_config(app).await?;
        let executable = self.select_executable(&state.enabled_names, tool_name)?;
        drop(state);
        match executable {
            ExecutableTool::OneShot(tool) => tool.execute(args, app).await,
            ExecutableTool::Interactive(tool) => tool.execute(args, app).await,
            ExecutableTool::Streaming(tool) => tool.read_current(app).map(Value::String),
        }
    }

    fn select_executable(
        &self,
        enabled_names: &HashSet<String>,
        tool_name: &str,
    ) -> Result<ExecutableTool, String> {
        let resolved_name = self
            .resolve_tool_name(tool_name)
            .ok_or_else(|| format!("工具未注册: {tool_name}"))?;
        if !enabled_names.contains(resolved_name) {
            return Err(format!("工具未在 allowlist 中启用: {tool_name}"));
        }
        let entry = self
            .tools
            .get(resolved_name)
            .ok_or_else(|| format!("工具未注册: {tool_name}"))?;
        Ok(match entry {
            ToolEntry::OneShot(tool) => ExecutableTool::OneShot(tool.clone()),
            ToolEntry::Interactive(tool) => ExecutableTool::Interactive(tool.clone()),
            ToolEntry::Streaming(tool) => ExecutableTool::Streaming(tool.clone()),
        })
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    async fn loaded_config(
        &self,
        app: &AppHandle,
    ) -> Result<tokio::sync::MutexGuard<'_, ConfigState>, String> {
        self.ensure_enabled_config_loaded(app).await?;
        Ok(self.enabled_config.lock().await)
    }
}

#[cfg(test)]
#[path = "tool_registry_tests.rs"]
mod tests;
