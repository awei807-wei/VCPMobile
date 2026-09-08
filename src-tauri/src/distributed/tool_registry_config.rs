use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::Manager;

pub(super) const ENABLED_CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ParsedConfig {
    Enabled(Vec<String>),
    LegacyDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfigSource {
    Unloaded,
    Default,
    Explicit,
    LegacyMigration,
}

impl ConfigSource {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Unloaded => "unloaded",
            Self::Default => "default",
            Self::Explicit => "explicit",
            Self::LegacyMigration => "legacyMigration",
        }
    }
}

#[derive(Debug)]
pub(super) struct ConfigState {
    pub(super) loaded: bool,
    pub(super) enabled_names: HashSet<String>,
    pub(super) source: ConfigSource,
    pub(super) last_error: Option<String>,
}

impl Default for ConfigState {
    fn default() -> Self {
        Self {
            loaded: false,
            enabled_names: HashSet::new(),
            source: ConfigSource::Unloaded,
            last_error: None,
        }
    }
}

/// 分布式工具 allowlist 的可观测状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfigStatus {
    pub loaded: bool,
    pub enabled_names: Vec<String>,
    pub source: String,
    pub last_error: Option<String>,
}

impl ConfigState {
    pub(super) fn status(&self) -> ToolConfigStatus {
        let mut enabled_names: Vec<_> = self.enabled_names.iter().cloned().collect();
        enabled_names.sort();
        ToolConfigStatus {
            loaded: self.loaded,
            enabled_names,
            source: self.source.label().to_string(),
            last_error: self.last_error.clone(),
        }
    }
}

pub(super) fn parse_config(content: &str) -> Result<ParsedConfig, String> {
    let value: Value =
        serde_json::from_str(content).map_err(|error| format!("配置解析失败: {error}"))?;
    if value.is_array() {
        parse_string_array(&value, "旧版 disabled 配置")?;
        return Ok(ParsedConfig::LegacyDisabled);
    }
    let object = value
        .as_object()
        .ok_or_else(|| "工具配置必须是对象或旧版数组".to_string())?;
    let version = object
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| "工具配置缺少 schemaVersion".to_string())?;
    match version {
        1 => {
            let disabled = object
                .get("disabledNames")
                .ok_or_else(|| "旧版工具配置缺少 disabledNames".to_string())?;
            parse_string_array(disabled, "旧版 disabled 配置")?;
            Ok(ParsedConfig::LegacyDisabled)
        }
        value if value == u64::from(ENABLED_CONFIG_SCHEMA_VERSION) => {
            let enabled = object
                .get("enabledNames")
                .ok_or_else(|| "allowlist 配置缺少 enabledNames".to_string())?;
            Ok(ParsedConfig::Enabled(parse_string_array(
                enabled,
                "enabled allowlist",
            )?))
        }
        value => Err(format!("不支持的工具配置 schemaVersion: {value}")),
    }
}

fn parse_string_array(value: &Value, label: &str) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{label} 必须是字符串数组"))?
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{label} 包含无效工具名"))
        })
        .collect()
}

pub(super) fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("distributed_tools.json"))
        .map_err(|error| format!("读取分布式工具配置目录失败: {error}"))
}

pub(super) fn read_config(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取分布式工具配置失败: {error}")),
    }
}

pub(super) fn persist_config(path: &Path, enabled_names: &HashSet<String>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "分布式工具配置路径缺少父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("创建分布式工具配置目录失败: {error}"))?;
    let mut names: Vec<_> = enabled_names.iter().cloned().collect();
    names.sort();
    let content = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": ENABLED_CONFIG_SCHEMA_VERSION,
        "enabledNames": names,
    }))
    .map_err(|error| format!("序列化分布式工具配置失败: {error}"))?;
    let temp_path = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)
        .map_err(|error| format!("创建分布式工具配置临时文件失败: {error}"))?;
    file.write_all(&content)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("写入分布式工具配置失败: {error}"))?;
    drop(file);
    fs::rename(&temp_path, path).map_err(|error| format!("发布分布式工具配置失败: {error}"))
}
