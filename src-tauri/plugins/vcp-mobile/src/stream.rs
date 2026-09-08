#[allow(unused_imports)]
use tauri::{AppHandle, Manager, Runtime};

#[allow(unused_imports)]
use crate::VcpMobileState;

/// 流式前台 lease 的可选所有权身份。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamIdentity {
    #[serde(default)]
    pub owner_type: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub topic_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
}

impl StreamIdentity {
    /// 构造用于隔离单条流式消息的完整身份。
    pub fn new(
        owner_type: impl Into<String>,
        owner_id: impl Into<String>,
        topic_id: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Self {
        Self {
            owner_type: Some(owner_type.into()),
            owner_id: Some(owner_id.into()),
            topic_id: Some(topic_id.into()),
            message_id: Some(message_id.into()),
        }
    }

    fn is_complete(&self) -> bool {
        self.owner_type
            .as_deref()
            .is_some_and(|value| matches!(value, "agent" | "group"))
            && self
                .owner_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .topic_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .message_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

/// 申请持有进程级前台锁（语义化接口）。
pub fn acquire_foreground_inner<R: Runtime>(
    _app: &AppHandle<R>,
    _tag: &str,
    priority: i32,
    _label: &str,
    screen_keep_on: bool,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "acquireForeground",
                serde_json::json!({
                    "tag": _tag,
                    "priority": priority,
                    "label": _label,
                    "screenKeepOn": screen_keep_on
                }),
            )
            .map_err(|e| format!("调用原生前台申请失败: {}", e))?;
    }

    log::info!(
        "[VcpMobilePlugin] acquire_foreground_inner: priority={}, screenKeepOn={}",
        priority,
        screen_keep_on
    );

    Ok(())
}

/// 释放进程级前台锁（语义化接口）。
pub fn release_foreground_inner<R: Runtime>(_app: &AppHandle<R>, _tag: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "releaseForeground",
                serde_json::json!({ "tag": _tag }),
            )
            .map_err(|e| format!("调用原生前台释放失败: {}", e))?;
    }

    log::info!("[VcpMobilePlugin] release_foreground_inner: result=ok");

    Ok(())
}

/// 清除 Android 启动恢复留下的全部分布式 native lease。
///
/// 该操作只针对 `distributed` 命名空间，不影响独立的流式 lease；桌面端
/// 没有对应 native 状态，因此保持幂等 no-op。
pub fn release_distributed_keepalive_inner<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = _app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle 未初始化")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>(
                "releaseDistributedForeground",
                serde_json::json!({}),
            )
            .map_err(|e| format!("调用原生分布式前台清理失败: {e}"))?;
    }

    log::info!("[VcpMobilePlugin] release_distributed_keepalive_inner: result=ok");
    Ok(())
}

/// 启动流式保活服务（兼容旧版接口）。
pub fn start_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    start_stream_service_with_identity_inner(app, agent_name, None).map(|_| ())
}

/// 使用完整所有权身份申请流式 lease。
pub fn start_stream_service_with_identity_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
    identity: Option<&StreamIdentity>,
) -> Result<Option<u64>, String> {
    reject_partial_identity(identity)?;
    #[cfg(target_os = "android")]
    if let Some(identity) = identity {
        return start_streaming_service_native(app, agent_name, identity).map(Some);
    }

    let tag = stream_lease_tag(agent_name, identity)?;
    let priority = if agent_name.contains("[数据同步]") {
        40
    } else if agent_name.contains("[预渲染重建]") {
        30
    } else {
        20
    };
    let screen_keep_on = agent_name.contains("[数据同步]") || agent_name.contains("[预渲染重建]");
    acquire_foreground_inner(app, &tag, priority, agent_name, screen_keep_on).map(|_| None)
}

/// 停止流式保活服务（兼容旧版接口）。
pub fn stop_stream_service_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
) -> Result<(), String> {
    stop_stream_service_with_identity_inner(app, agent_name, None, None)
}

/// 释放指定 generation 的流式 lease；完整身份不得省略 generation。
pub fn stop_stream_service_with_identity_inner<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
    identity: Option<&StreamIdentity>,
    expected_generation: Option<u64>,
) -> Result<(), String> {
    reject_partial_identity(identity)?;
    let generation = expected_generation_for_identity(identity, expected_generation)?;
    #[cfg(target_os = "android")]
    if let (Some(identity), Some(generation)) = (identity, generation) {
        return stop_streaming_service_native(app, agent_name, identity, generation);
    }
    #[cfg(not(target_os = "android"))]
    let _ = generation;
    let tag = stream_lease_tag(agent_name, identity)?;
    release_foreground_inner(app, &tag)
}

#[cfg(target_os = "android")]
fn start_streaming_service_native<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
    identity: &StreamIdentity,
) -> Result<u64, String> {
    let state = app.state::<VcpMobileState<R>>();
    let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
    let plugin_handle = handle.as_ref().ok_or("Plugin handle 未初始化")?;
    let response = plugin_handle
        .run_mobile_plugin::<serde_json::Value>(
            "startStreamingService",
            serde_json::json!({
                "agentName": agent_name,
                "ownerType": identity.owner_type.as_deref(),
                "ownerId": identity.owner_id.as_deref(),
                "topicId": identity.topic_id.as_deref(),
                "messageId": identity.message_id.as_deref()
            }),
        )
        .map_err(|e| format!("调用原生 startStreamingService 失败: {e}"))?;
    parse_native_generation(&response, "startStreamingService")
}

#[cfg(target_os = "android")]
fn stop_streaming_service_native<R: Runtime>(
    app: &AppHandle<R>,
    agent_name: &str,
    identity: &StreamIdentity,
    expected_generation: u64,
) -> Result<(), String> {
    let state = app.state::<VcpMobileState<R>>();
    let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
    let plugin_handle = handle.as_ref().ok_or("Plugin handle 未初始化")?;
    plugin_handle
        .run_mobile_plugin::<serde_json::Value>(
            "stopStreamingService",
            serde_json::json!({
                "agentName": agent_name,
                "ownerType": identity.owner_type.as_deref(),
                "ownerId": identity.owner_id.as_deref(),
                "topicId": identity.topic_id.as_deref(),
                "messageId": identity.message_id.as_deref(),
                "expectedGeneration": expected_generation
            }),
        )
        .map_err(|e| format!("调用原生 stopStreamingService 失败: {e}"))?;
    Ok(())
}

#[allow(dead_code)]
fn parse_native_generation(response: &serde_json::Value, operation: &str) -> Result<u64, String> {
    response
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .filter(|generation| *generation > 0)
        .ok_or_else(|| format!("原生 {operation} 未返回有效 generation"))
}

fn expected_generation_for_identity(
    identity: Option<&StreamIdentity>,
    expected_generation: Option<u64>,
) -> Result<Option<u64>, String> {
    if identity.is_none() {
        return Ok(None);
    }
    expected_generation
        .filter(|value| *value > 0)
        .map(Some)
        .ok_or_else(|| "完整身份释放必须提供正整数 generation".to_string())
}

fn reject_partial_identity(identity: Option<&StreamIdentity>) -> Result<(), String> {
    if identity.is_some_and(|value| !value.is_complete()) {
        return Err(
            "流式前台 lease 身份不完整，必须同时提供 ownerType、ownerId、topicId、messageId"
                .to_string(),
        );
    }
    Ok(())
}

fn stream_lease_tag(agent_name: &str, identity: Option<&StreamIdentity>) -> Result<String, String> {
    reject_partial_identity(identity)?;
    let Some(identity) = identity else {
        return Ok(format!("stream:{}", agent_name));
    };
    Ok(format!(
        "stream:session:{}:{}:{}:{}",
        encode_tag_part(identity.owner_type.as_deref().unwrap_or_default()),
        encode_tag_part(identity.owner_id.as_deref().unwrap_or_default()),
        encode_tag_part(identity.topic_id.as_deref().unwrap_or_default()),
        encode_tag_part(identity.message_id.as_deref().unwrap_or_default())
    ))
}

fn encode_tag_part(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 设置分布式保活模式 (兼容老版本接口)
pub fn set_keepalive_mode_inner<R: Runtime>(
    app: &AppHandle<R>,
    is_keepalive: bool,
) -> Result<(), String> {
    if is_keepalive {
        acquire_foreground_inner(app, "distributed", 10, "distributed", false)
    } else {
        release_foreground_inner(app, "distributed")
    }
}

#[tauri::command]
pub fn acquire_foreground<R: Runtime>(
    app: AppHandle<R>,
    tag: String,
    priority: i32,
    label: String,
    screen_keep_on: bool,
) -> Result<(), String> {
    acquire_foreground_inner(&app, &tag, priority, &label, screen_keep_on)
}

#[tauri::command]
pub fn release_foreground<R: Runtime>(app: AppHandle<R>, tag: String) -> Result<(), String> {
    release_foreground_inner(&app, &tag)
}

#[tauri::command]
pub fn start_streaming_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: String,
    identity: Option<StreamIdentity>,
) -> Result<Option<u64>, String> {
    start_stream_service_with_identity_inner(&app, &agent_name, identity.as_ref())
}

#[tauri::command]
pub fn stop_streaming_service<R: Runtime>(
    app: AppHandle<R>,
    agent_name: Option<String>,
    identity: Option<StreamIdentity>,
    expected_generation: Option<u64>,
) -> Result<(), String> {
    stop_stream_service_with_identity_inner(
        &app,
        agent_name.as_deref().unwrap_or_default(),
        identity.as_ref(),
        expected_generation,
    )
}
#[tauri::command]
pub fn set_keepalive_mode<R: Runtime>(app: AppHandle<R>, is_keepalive: bool) -> Result<(), String> {
    set_keepalive_mode_inner(&app, is_keepalive)
}

#[tauri::command]
#[allow(unused_variables)]
pub fn start_helper_service<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let state = app.state::<VcpMobileState<R>>();
        let handle = state.plugin_handle.lock().map_err(|e| e.to_string())?;
        let plugin_handle = handle.as_ref().ok_or("Plugin handle not initialized")?;
        plugin_handle
            .run_mobile_plugin::<serde_json::Value>("startHelperService", serde_json::json!({}))
            .map_err(|e| format!("调用原生 helper 服务失败: {}", e))?;
    }
    log::info!("[VcpMobilePlugin] start_helper_service called");
    Ok(())
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
