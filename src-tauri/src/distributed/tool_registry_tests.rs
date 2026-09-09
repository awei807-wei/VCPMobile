use super::config::{parse_config, persist_config, ParsedConfig};
use super::{OneShotTool, ToolRegistry};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tauri::AppHandle;

struct NoopTool(&'static str);

#[async_trait]
impl OneShotTool for NoopTool {
    fn manifest(&self) -> super::super::types::ToolManifest {
        super::super::types::ToolManifest {
            name: self.0.to_string(),
            display_name: self.0.to_string(),
            description: String::new(),
            placeholder: None,
            invocation_commands: Vec::new(),
            web_socket_push: None,
        }
    }

    async fn execute(&self, _args: Value, _app: &AppHandle) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

#[test]
fn 初始和未加载策略默认拒绝全部工具() {
    let mut registry = ToolRegistry::new();
    registry.register_oneshot(NoopTool("KnownTool"));
    assert!(!registry.is_enabled("KnownTool"));
    assert!(!registry.is_enabled("UnknownTool"));
}

#[test]
fn 旧版disabled配置安全迁移为空allowlist() {
    assert_eq!(
        parse_config(r#"["KnownTool"]"#).unwrap(),
        ParsedConfig::LegacyDisabled
    );
    assert_eq!(
        parse_config(r#"{"schemaVersion":1,"disabledNames":["KnownTool"]}"#).unwrap(),
        ParsedConfig::LegacyDisabled
    );
}

#[test]
fn 显式allowlist拒绝未知项和重复项() {
    let mut registry = ToolRegistry::new();
    registry.register_oneshot(NoopTool("KnownTool"));
    assert!(registry
        .validate_enabled_names(vec!["KnownTool".to_string()])
        .unwrap()
        .contains("KnownTool"));
    assert!(registry
        .validate_enabled_names(vec!["UnknownTool".to_string()])
        .is_err());
    assert!(registry
        .validate_enabled_names(vec!["KnownTool".to_string(), "KnownTool".to_string()])
        .is_err());
}

#[test]
fn 损坏或未知版本配置拒绝加载() {
    assert!(parse_config("{").is_err());
    assert!(parse_config(r#"{"schemaVersion":99,"enabledNames":[]}"#).is_err());
}

#[test]
fn 配置落盘使用显式enabled_names() {
    let path = std::env::temp_dir().join(format!(
        "vcp-distributed-tools-test-{}-{}.json",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let names: HashSet<String> = ["KnownTool".to_string()].into_iter().collect();
    persist_config(&path, &names).expect("配置应成功落盘");
    let content = fs::read_to_string(&path).expect("应读取已发布配置");
    assert_eq!(
        parse_config(&content).unwrap(),
        ParsedConfig::Enabled(vec!["KnownTool".to_string()])
    );
    let _ = fs::remove_file(path);
}

#[test]
fn 配置落盘失败不会覆盖原文件() {
    let root = std::env::temp_dir().join(format!(
        "vcp-distributed-tools-blocked-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::write(&root, "占位文件").expect("创建阻塞路径");
    let path = root.join("distributed_tools.json");
    let names: HashSet<String> = ["KnownTool".to_string()].into_iter().collect();
    assert!(persist_config(Path::new(&path), &names).is_err());
    assert_eq!(fs::read_to_string(&root).unwrap(), "占位文件");
    let _ = fs::remove_file(root);
}

#[test]
fn 配置持久化失败会记录到状态且不发布新allowlist() {
    let registry = ToolRegistry::new();
    let mut state = super::config::ConfigState::default();
    state.enabled_names.insert("原有工具".to_string());
    let result: Result<bool, String> =
        registry.remember_persist_error(&mut state, "模拟持久化失败".to_string());

    assert_eq!(result.unwrap_err(), "模拟持久化失败");
    assert_eq!(state.enabled_names.len(), 1);
    assert_eq!(state.status().last_error.as_deref(), Some("模拟持久化失败"));
}
