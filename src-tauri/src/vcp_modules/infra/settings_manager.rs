// SettingsManager: 处理应用全局配置的核心模块
// 职责: 管理全局配置，实现基于 SQLite 的原子写入与并发控制。

use crate::vcp_modules::db_manager::DbState;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

fn default_sync_log_level() -> String {
    "INFO".to_string()
}

fn default_active_connection_profile_id() -> String {
    "lan".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vcp_server_url: String,
    #[serde(default)]
    pub vcp_api_key: String,
    #[serde(default)]
    pub vcp_log_url: String,
    #[serde(default)]
    pub vcp_log_key: String,
    #[serde(default)]
    pub sync_server_url: String,
    #[serde(default)]
    pub sync_http_url: String,
    #[serde(default)]
    pub sync_token: String,
    #[serde(default)]
    pub distributed_ws_url: String,
    #[serde(default)]
    pub distributed_vcp_key: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub user_name: String,

    // 分布式设置
    #[serde(default)]
    pub distributed_enabled: bool,
    #[serde(default)]
    pub distributed_ws_url: String,
    #[serde(default)]
    pub distributed_vcp_key: String,
    #[serde(default)]
    pub distributed_device_name: String,

    // VCP 核心服务器
    #[serde(default)]
    pub vcp_server_url: String,
    #[serde(default)]
    pub vcp_api_key: String,
    #[serde(default)]
    pub vcp_log_url: String,
    #[serde(default)]
    pub vcp_log_key: String,

    // VCP 数据同步连接
    #[serde(default)]
    pub sync_server_url: String, // WebSocket 服务 URL (ws://ip:port)
    #[serde(default)]
    pub sync_http_url: String, // HTTP API 服务 URL (http://ip:port)
    #[serde(default)]
    pub sync_token: String,

    // 管理接口鉴权 (用于表情包刷新等)
    #[serde(default)]
    pub admin_username: String,
    #[serde(default)]
    pub admin_password: String,

    // 表情包图床密钥
    #[serde(default)]
    pub file_key: String,

    // 话题总结配置
    #[serde(default)]
    pub topic_summary_model: String,

    // 同步日志配置
    #[serde(default = "default_sync_log_level")]
    pub sync_log_level: String,

    // 排序逻辑 (移动端分组)
    #[serde(default)]
    pub agent_order: Vec<String>,
    #[serde(default)]
    pub group_order: Vec<String>,

    #[serde(default)]
    pub current_theme_mode: Option<String>,

    /// 同步时是否执行消息预渲染（默认 false，节省同步时间）
    #[serde(default)]
    pub sync_prerender_enabled: bool,

    #[serde(default)]
    pub enable_assistant: bool,

    #[serde(default)]
    pub assistant_agent_id: String,

    #[serde(default)]
    pub connection_profiles: Vec<ConnectionProfile>,

    #[serde(default = "default_active_connection_profile_id")]
    pub active_connection_profile_id: String,

    /// 仅保留此字段用于前端未来扩展的透参
    #[serde(flatten)]
    #[serde(default)]
    pub extra: serde_json::Value,
}

#[derive(Default)]
pub struct ConnectionProfileSwitchState {
    switching: AtomicBool,
}

impl ConnectionProfileSwitchState {
    pub fn begin(&self) {
        self.switching.store(true, Ordering::SeqCst);
    }

    pub fn end(&self) {
        self.switching.store(false, Ordering::SeqCst);
    }

    pub fn is_switching(&self) -> bool {
        self.switching.load(Ordering::SeqCst)
    }
}

pub fn is_connection_profile_switching<R: Runtime>(app_handle: &AppHandle<R>) -> bool {
    app_handle
        .try_state::<ConnectionProfileSwitchState>()
        .map(|state| state.is_switching())
        .unwrap_or(false)
}

#[tauri::command]
pub async fn begin_connection_profile_switch(
    state: State<'_, ConnectionProfileSwitchState>,
) -> Result<(), String> {
    state.begin();
    Ok(())
}

#[tauri::command]
pub async fn end_connection_profile_switch(
    state: State<'_, ConnectionProfileSwitchState>,
) -> Result<(), String> {
    state.end();
    Ok(())
}

#[tauri::command]
pub async fn is_connection_profile_switching_command(
    state: State<'_, ConnectionProfileSwitchState>,
) -> Result<bool, String> {
    Ok(state.is_switching())
}

pub struct SettingsState {
    pub cache: Arc<Mutex<Option<Settings>>>,
    pub lock: Arc<Mutex<()>>,
}

impl SettingsState {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
            lock: Arc::new(Mutex::new(())),
        }
    }
}

pub fn create_default_settings() -> Settings {
    Settings {
        user_name: "用户".to_string(),
        distributed_enabled: false,
        distributed_ws_url: "".to_string(),
        distributed_vcp_key: "".to_string(),
        distributed_device_name: "VCPMobile".to_string(),
        vcp_server_url: "".to_string(),
        vcp_api_key: "".to_string(),
        vcp_log_url: "".to_string(),
        vcp_log_key: "".to_string(),
        sync_server_url: "".to_string(),
        sync_http_url: "".to_string(),
        sync_token: "".to_string(),
        admin_username: "".to_string(),
        admin_password: "".to_string(),
        file_key: "".to_string(),
        topic_summary_model: "gemini-2.5-flash".to_string(),
        sync_log_level: "INFO".to_string(),
        sync_prerender_enabled: false,
        enable_assistant: false,
        assistant_agent_id: "".to_string(),
        connection_profiles: vec![],
        active_connection_profile_id: default_active_connection_profile_id(),
        agent_order: vec![],
        group_order: vec![],
        current_theme_mode: Some("dark".to_string()),
        extra: serde_json::Value::Object(serde_json::Map::new()),
    }
}

fn migrate_realtime_profile_aliases(profile: &mut ConnectionProfile) {
    if profile.vcp_log_url.trim().is_empty()
        && profile.vcp_log_key.trim().is_empty()
        && (!profile.distributed_ws_url.trim().is_empty()
            || !profile.distributed_vcp_key.trim().is_empty())
    {
        profile.vcp_log_url = profile.distributed_ws_url.clone();
        profile.vcp_log_key = profile.distributed_vcp_key.clone();
    }
}

fn mirror_realtime_profile_aliases(profile: &mut ConnectionProfile) {
    profile.distributed_ws_url = profile.vcp_log_url.clone();
    profile.distributed_vcp_key = profile.vcp_log_key.clone();
}

fn migrate_realtime_aliases(settings: &mut Settings) {
    if settings.vcp_log_url.trim().is_empty()
        && settings.vcp_log_key.trim().is_empty()
        && (!settings.distributed_ws_url.trim().is_empty()
            || !settings.distributed_vcp_key.trim().is_empty())
    {
        settings.vcp_log_url = settings.distributed_ws_url.clone();
        settings.vcp_log_key = settings.distributed_vcp_key.clone();
    }

    for profile in &mut settings.connection_profiles {
        migrate_realtime_profile_aliases(profile);
    }
}

fn mirror_realtime_aliases(settings: &mut Settings) {
    settings.distributed_ws_url = settings.vcp_log_url.clone();
    settings.distributed_vcp_key = settings.vcp_log_key.clone();

    for profile in &mut settings.connection_profiles {
        mirror_realtime_profile_aliases(profile);
    }
}

#[tauri::command]
pub async fn read_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
) -> Result<Settings, String> {
    if let Some(cached) = &*state.cache.lock().await {
        return Ok(cached.clone());
    }

    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let row_res = sqlx::query("SELECT value FROM settings WHERE key = 'global'")
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;

    let mut settings = if let Some(row) = row_res {
        use sqlx::Row;
        let content: String = row.get("value");
        serde_json::from_str(&content).unwrap_or_else(|_| create_default_settings())
    } else {
        create_default_settings()
    };

    migrate_realtime_aliases(&mut settings);
    *state.cache.lock().await = Some(settings.clone());
    Ok(settings)
}

#[tauri::command]
pub async fn write_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    mut settings: Settings,
) -> Result<bool, String> {
    let _lock = state.lock.lock().await;
    mirror_realtime_aliases(&mut settings);
    internal_write_settings(&app_handle, &state, &settings).await
}

#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    updates: serde_json::Value,
) -> Result<Settings, String> {
    let _lock = state.lock.lock().await;

    let current = read_settings(app_handle.clone(), state.clone()).await?;
    let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;

    if let Some(obj) = updates.as_object() {
        if let Some(current_obj) = current_val.as_object_mut() {
            for (k, v) in obj {
                current_obj.insert(k.clone(), v.clone());
            }
        }
    }

    let mut new_settings: Settings =
        serde_json::from_value(current_val).map_err(|e| e.to_string())?;
    mirror_realtime_aliases(&mut new_settings);
    internal_write_settings(&app_handle, &state, &new_settings).await?;

    Ok(new_settings)
}

async fn internal_write_settings<R: Runtime>(
    app_handle: &AppHandle<R>,
    state: &SettingsState,
    settings: &Settings,
) -> Result<bool, String> {
    let db_state = app_handle.state::<DbState>();
    let pool = &db_state.pool;

    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    let now = crate::vcp_modules::infra::utils::now_millis();

    sqlx::query("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('global', ?, ?)")
        .bind(&content)
        .bind(now)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    // 判断 VCPLog 连接参数是否实际发生变化，避免无关设置（如排序顺序）更新导致重连
    let should_reconnect = {
        let old_cache = state.cache.lock().await;
        if let Some(ref old) = *old_cache {
            old.vcp_log_url != settings.vcp_log_url || old.vcp_log_key != settings.vcp_log_key
        } else {
            !settings.vcp_log_url.is_empty() || !settings.vcp_log_key.is_empty()
        }
    };

    // 判断分布式设置是否发生改变
    let (should_reconcile_dist, force_reconnect_dist) = {
        let old_cache = state.cache.lock().await;
        if let Some(ref old) = *old_cache {
            let enabled_changed = old.distributed_enabled != settings.distributed_enabled;
            let params_changed = old.distributed_ws_url != settings.distributed_ws_url
                || old.distributed_vcp_key != settings.distributed_vcp_key
                || old.distributed_device_name != settings.distributed_device_name;

            let should = enabled_changed || (params_changed && settings.distributed_enabled);
            let force = params_changed && settings.distributed_enabled;
            (should, force)
        } else {
            (settings.distributed_enabled, false)
        }
    };

    *state.cache.lock().await = Some(settings.clone());

    // [强耦合联动] 仅当 VCPLog 连接参数实际变化时，才通知 VCP Log 服务更新连接状态
    if should_reconnect {
        let h = app_handle.clone();
        let log_url = settings.vcp_log_url.clone();
        let log_key = settings.vcp_log_key.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::vcp_modules::vcp_log_service::init_vcp_log_connection_internal(
                h.clone(),
                log_url.clone(),
                log_key.clone(),
            )
            .await;
            let _ = crate::vcp_modules::vcp_info_service::init_vcp_info_connection_internal(
                h, log_url, log_key,
            )
            .await;
        });
    }

    // 分布式生命周期自动联动
    if should_reconcile_dist {
        let concrete_app = app_handle.state::<tauri::AppHandle>().inner().clone();
        let enabled = settings.distributed_enabled;
        crate::vcp_modules::infra::lifecycle_manager::reconcile_distributed_node(
            &concrete_app,
            enabled,
            force_reconnect_dist,
        )
        .await;
    }

    Ok(true)
}

#[tauri::command]
pub async fn set_theme<R: Runtime>(
    app_handle: AppHandle<R>,
    state: State<'_, SettingsState>,
    theme: String,
) -> Result<bool, String> {
    let updates = serde_json::json!({
        "currentThemeMode": theme
    });

    update_settings(app_handle, state, updates).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_realtime_aliases_keeps_vcp_log_as_source_of_truth() {
        let mut settings = create_default_settings();
        settings.vcp_log_url = "wss://example.com".to_string();
        settings.vcp_log_key = "log-key".to_string();
        settings.distributed_ws_url = "ws://stale.local".to_string();
        settings.distributed_vcp_key = "stale-key".to_string();

        mirror_realtime_aliases(&mut settings);

        assert_eq!(settings.distributed_ws_url, "wss://example.com");
        assert_eq!(settings.distributed_vcp_key, "log-key");
    }

    #[test]
    fn migrate_realtime_aliases_backfills_from_legacy_distributed_fields() {
        let mut settings = create_default_settings();
        settings.distributed_ws_url = "ws://legacy.local:6005".to_string();
        settings.distributed_vcp_key = "legacy-key".to_string();

        migrate_realtime_aliases(&mut settings);

        assert_eq!(settings.vcp_log_url, "ws://legacy.local:6005");
        assert_eq!(settings.vcp_log_key, "legacy-key");
    }

    #[test]
    fn mirror_realtime_aliases_preserves_explicit_clears() {
        let mut settings = create_default_settings();
        settings.distributed_ws_url = "ws://legacy.local:6005".to_string();
        settings.distributed_vcp_key = "legacy-key".to_string();

        mirror_realtime_aliases(&mut settings);

        assert!(settings.vcp_log_url.is_empty());
        assert!(settings.vcp_log_key.is_empty());
        assert!(settings.distributed_ws_url.is_empty());
        assert!(settings.distributed_vcp_key.is_empty());
    }
}
