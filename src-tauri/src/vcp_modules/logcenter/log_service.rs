//! Fork read-only adaptation of upstream logcenter/log_service.rs.
//! Commands accept an expected connection identity, never caller-controlled credentials.
use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{
    is_connection_profile_switching, read_settings, Settings, SettingsState,
};
use futures_util::StreamExt;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

const FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFetchResult {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub need_full_reload: bool,
}

fn validate_connection_scope(
    settings: &Settings,
    expected_profile_id: &str,
    expected_server_url: &str,
) -> Result<(), String> {
    if settings.active_connection_profile_id != expected_profile_id {
        return Err("CONNECTION_CHANGED: 线路已变化，请重新加载日志".to_string());
    }
    let actual = admin_api::normalize_server_base(&settings.vcp_server_url)?;
    let expected = admin_api::normalize_server_base(expected_server_url)?;
    if actual != expected {
        return Err("CONNECTION_CHANGED: 服务器地址已变化，请重新加载日志".to_string());
    }
    Ok(())
}

fn append_capped_bytes(buffer: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(buffer.len()) {
        return Err("日志响应超过 8 MiB，已停止读取".to_string());
    }
    buffer.extend_from_slice(chunk);
    Ok(())
}

async fn read_capped_text(resp: reqwest::Response) -> Result<String, String> {
    if resp
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return Err("日志响应超过 8 MiB，已拒绝读取".to_string());
    }
    let mut buffer = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "日志响应读取失败".to_string())?;
        append_capped_bytes(&mut buffer, &chunk)?;
    }
    String::from_utf8(buffer).map_err(|_| "服务器返回了非 UTF-8 响应".to_string())
}

#[tauri::command]
pub async fn logcenter_fetch<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    incremental: bool,
    offset: u64,
    expected_profile_id: String,
    expected_server_url: String,
) -> Result<LogFetchResult, String> {
    if offset > MAX_JS_SAFE_INTEGER {
        return Err("日志游标超出安全范围".to_string());
    }
    if is_connection_profile_switching(&app_handle) {
        return Err("CONNECTION_SWITCHING: 正在切换线路".to_string());
    }
    let settings = read_settings(app_handle.clone(), settings_state).await?;
    admin_api::ensure_admin_config(&settings, "日志中心")?;
    validate_connection_scope(&settings, &expected_profile_id, &expected_server_url)?;
    if is_connection_profile_switching(&app_handle) {
        return Err("CONNECTION_SWITCHING: 正在切换线路".to_string());
    }

    // The target is derived ONLY from the backend Settings snapshot.
    let mut request = admin_api::admin_request(&settings, Method::GET, &["server-log"])?;
    if incremental {
        request = request.query(&[
            ("incremental", "true".to_string()),
            ("offset", offset.to_string()),
        ]);
    }
    let resp = request
        .timeout(FETCH_TOTAL_TIMEOUT)
        .send()
        .await
        .map_err(|_| "日志拉取失败，请检查连接或稍后重试".to_string())?;
    match resp.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            return Err("管理员凭据校验失败，请检查当前配置".to_string())
        }
        StatusCode::NOT_FOUND => return Err("日志文件不存在，或服务器不支持日志接口".to_string()),
        StatusCode::SERVICE_UNAVAILABLE => return Err("日志服务暂不可用，请稍后重试".to_string()),
        status => return Err(format!("日志拉取失败: HTTP {}", status.as_u16())),
    }
    let body = read_capped_text(resp).await?;
    let mut result: LogFetchResult =
        serde_json::from_str(&body).map_err(|_| "服务器日志响应不符合 JSON 契约".to_string())?;
    if result.offset > MAX_JS_SAFE_INTEGER || result.file_size > MAX_JS_SAFE_INTEGER {
        return Err("日志响应中的大小或游标超出安全范围".to_string());
    }
    // Retain upstream's boundary replacement-character behavior.
    result.content = result.content.trim_matches('\u{FFFD}').to_string();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            active_connection_profile_id: "lan".to_string(),
            vcp_server_url: "https://example.test/vcp/v1/chat/completions".to_string(),
            admin_username: "test-admin".to_string(),
            admin_password: "not-a-real-secret".to_string(),
            ..Default::default()
        }
    }
    #[test]
    fn same_profile_and_normalized_server_are_accepted() {
        assert!(validate_connection_scope(&settings(), "lan", "https://example.test/vcp/").is_ok());
    }
    #[test]
    fn different_profile_is_rejected() {
        assert!(
            validate_connection_scope(&settings(), "wan", "https://example.test/vcp/").is_err()
        );
    }
    #[test]
    fn different_server_is_rejected() {
        assert!(validate_connection_scope(&settings(), "lan", "https://other.test/vcp/").is_err());
    }
    #[test]
    fn embedded_credentials_are_rejected() {
        assert!(validate_connection_scope(
            &settings(),
            "lan",
            "https://user:pass@example.test/vcp/"
        )
        .is_err());
    }
    #[test]
    fn oversized_chunk_is_not_appended() {
        let mut bytes = vec![0; MAX_RESPONSE_BYTES - 1];
        assert!(append_capped_bytes(&mut bytes, &[1, 2]).is_err());
        assert_eq!(bytes.len(), MAX_RESPONSE_BYTES - 1);
    }
    #[test]
    fn utf8_split_across_chunks_is_preserved() {
        let source = "中文日志".as_bytes();
        let mut bytes = vec![];
        append_capped_bytes(&mut bytes, &source[..2]).unwrap();
        append_capped_bytes(&mut bytes, &source[2..]).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "中文日志");
    }
    #[test]
    fn rotation_notice_has_safe_defaults() {
        let result: LogFetchResult =
            serde_json::from_str(r#"{"needFullReload":true,"offset":0}"#).unwrap();
        assert!(result.need_full_reload);
        assert_eq!(result.file_size, 0);
    }
}
