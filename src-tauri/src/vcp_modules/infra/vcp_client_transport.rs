use serde_json::{json, Value};
use std::path::Path;

/// helper 端点的完整凭据；token 只在内存中传递，禁止 Debug 输出。
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct HelperEndpoint {
    pub(crate) port: u16,
    pub(crate) token: String,
}

/// 严格解析 version=1 的 helper endpoint，旧纯端口文件直接拒绝。
pub(crate) fn parse_helper_endpoint(content: &str) -> Result<HelperEndpoint, String> {
    let value: Value =
        serde_json::from_str(content).map_err(|_| "helper endpoint 不是有效 JSON".to_string())?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "helper endpoint 缺少 version".to_string())?;
    if version != 1 {
        return Err("helper endpoint version 不受支持".to_string());
    }
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .filter(|port| (1..=u16::MAX as u64).contains(port))
        .map(|port| port as u16)
        .ok_or_else(|| "helper endpoint port 无效".to_string())?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| "helper endpoint token 缺失".to_string())?;
    Ok(HelperEndpoint {
        port,
        token: token.to_string(),
    })
}

pub(crate) fn read_helper_endpoint(path: &Path) -> Result<HelperEndpoint, String> {
    let content = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse_helper_endpoint(&content)
}

/// 生成 helper 命令；调用方附加的身份/token 字段始终由已验证值覆盖。
pub(crate) fn serialize_helper_command(
    action: &str,
    request_id: &str,
    owner_type: &str,
    owner_id: &str,
    topic_id: &str,
    extra_params: Option<&Value>,
    token: &str,
) -> Result<String, String> {
    if token.trim().is_empty() {
        return Err("helper endpoint token 缺失".to_string());
    }
    let mut command = json!({
        "action": action,
        "requestId": request_id,
        "messageId": request_id,
        "ownerType": owner_type,
        "ownerId": owner_id,
        "topicId": topic_id,
    });
    if let Some(params) = extra_params.and_then(Value::as_object) {
        if let Some(object) = command.as_object_mut() {
            for (key, value) in params {
                object.insert(key.clone(), value.clone());
            }
            object.insert("requestId".into(), json!(request_id));
            object.insert("messageId".into(), json!(request_id));
            object.insert("ownerType".into(), json!(owner_type));
            object.insert("ownerId".into(), json!(owner_id));
            object.insert("topicId".into(), json!(topic_id));
            object.insert("token".into(), json!(token));
        }
    } else if let Some(object) = command.as_object_mut() {
        object.insert("token".into(), json!(token));
    }
    Ok(command.to_string())
}

#[cfg(target_os = "android")]
use crate::vcp_modules::chat::topic_types::MessageKey;
#[cfg(target_os = "android")]
use futures_util::StreamExt;
#[cfg(target_os = "android")]
use std::time::Duration;
#[cfg(target_os = "android")]
use tauri::{AppHandle, Manager, Runtime};
#[cfg(target_os = "android")]
use tokio_util::codec::{FramedRead, LengthDelimitedCodec};

#[cfg(target_os = "android")]
const HELPER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(target_os = "android")]
const HELPER_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(target_os = "android")]
pub(crate) fn get_helper_endpoint<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<HelperEndpoint, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");
    read_helper_endpoint(&port_file).map_err(|error| format!("sse_helper.port 不可用: {error}"))
}

#[cfg(target_os = "android")]
pub(crate) fn get_helper_port<R: Runtime>(app: &AppHandle<R>) -> Result<u16, String> {
    get_helper_endpoint(app).map(|endpoint| endpoint.port)
}

#[cfg(target_os = "android")]
pub(crate) async fn connect_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    action: &str,
    request_key: &MessageKey,
    extra_params: Option<Value>,
) -> Result<tokio::net::TcpStream, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");

    // 1. 尝试使用已有的端口文件进行连接（适用于 helper 已经在运行且就绪的情况）
    if let Some((stream, endpoint)) = connect_existing_helper(&port_file).await {
        log::info!("[VCPClient] Connected to existing sse helper socket");
        return send_command_to_stream(stream, action, request_key, extra_params, &endpoint.token)
            .await;
    }

    // 2. 如果连接失败或文件不存在，启动/唤醒 helper 服务
    log::info!(
        "[VCPClient] Helper not responding or port file missing. Starting/Waking helper service..."
    );
    let _ = tauri_plugin_vcp_mobile::stream::start_helper_service(app.clone());

    // 3. 循环等待新端口文件并尝试连接（最多尝试 60 次，每次间隔 50ms，总计 3 秒超时）
    let (stream, endpoint) = wait_for_helper(&port_file).await?;
    send_command_to_stream(stream, action, request_key, extra_params, &endpoint.token).await
}

#[cfg(target_os = "android")]
async fn connect_existing_helper(
    port_file: &Path,
) -> Option<(tokio::net::TcpStream, HelperEndpoint)> {
    let endpoint = read_helper_endpoint(port_file).ok()?;
    tokio::time::timeout(
        HELPER_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", endpoint.port)),
    )
    .await
    .ok()?
    .ok()
    .map(|stream| (stream, endpoint))
}

#[cfg(target_os = "android")]
async fn wait_for_helper(
    port_file: &Path,
) -> Result<(tokio::net::TcpStream, HelperEndpoint), String> {
    let max_attempts = 60;
    let delay = Duration::from_millis(50);
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let Some(endpoint) = read_helper_endpoint_for_retry(port_file, &mut last_err) else {
            tokio::time::sleep(delay).await;
            continue;
        };
        match tokio::time::timeout(
            HELPER_CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect(format!("127.0.0.1:{}", endpoint.port)),
        )
        .await
        {
            Ok(Ok(stream)) => {
                log::info!(
                    "[VCPClient] Connected to sse helper socket on 127.0.0.1:{} after {} attempts",
                    endpoint.port,
                    attempt
                );
                return Ok((stream, endpoint));
            }
            Ok(Err(error)) => {
                last_err = format!("Connect to helper port {} failed: {error}", endpoint.port);
                tokio::time::sleep(delay).await;
            }
            Err(_) => {
                last_err = format!("Connect to helper port {} timed out", endpoint.port);
                tokio::time::sleep(delay).await;
            }
        }
    }
    Err(format!(
        "Failed to connect to sse helper after {}s (last error: {})",
        max_attempts as f32 * 0.05,
        last_err
    ))
}

#[cfg(target_os = "android")]
fn read_helper_endpoint_for_retry(path: &Path, last_err: &mut String) -> Option<HelperEndpoint> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            *last_err = format!("Read port file error: {error}");
            return None;
        }
    };
    match parse_helper_endpoint(&content) {
        Ok(endpoint) => Some(endpoint),
        Err(error) => {
            *last_err = format!("Parse helper endpoint error: {error}");
            None
        }
    }
}

// 辅助函数：向已连接的 TcpStream 发送 JSON 指令
#[cfg(target_os = "android")]
pub(crate) async fn send_command_to_stream(
    mut stream: tokio::net::TcpStream,
    action: &str,
    request_key: &MessageKey,
    extra_params: Option<Value>,
    token: &str,
) -> Result<tokio::net::TcpStream, String> {
    use tokio::io::AsyncWriteExt;
    let cmd_str = serialize_helper_command(
        action,
        &request_key.msg_id,
        &request_key.topic.owner_type,
        &request_key.topic.owner_id,
        &request_key.topic.topic_id,
        extra_params.as_ref(),
        token,
    )?;
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
    tokio::time::timeout(HELPER_COMMAND_TIMEOUT, async {
        stream
            .write_all(&len.to_be_bytes())
            .await
            .map_err(|e| format!("Write command length error: {}", e))?;
        stream
            .write_all(cmd_bytes)
            .await
            .map_err(|e| format!("Write command error: {}", e))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("Flush command error: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "写入 helper 命令超时".to_string())??;
    Ok(stream)
}

#[cfg(target_os = "android")]
pub(crate) async fn send_stop_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    msg_id: &str,
    request_key: &MessageKey,
    expected_generation: u64,
) -> Result<(), String> {
    if msg_id != request_key.msg_id {
        return Err("helper stop 的 requestId 与已验证 messageId 不一致".to_string());
    }
    let endpoint = get_helper_endpoint(app)?;
    let mut stream = tokio::time::timeout(
        HELPER_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", endpoint.port)),
    )
    .await
    .map_err(|_| "连接 helper stop socket 超时".to_string())?
    .map_err(|e| e.to_string())?;

    use tokio::io::AsyncWriteExt;
    let cmd_str = serialize_helper_command(
        "stop",
        &request_key.msg_id,
        &request_key.topic.owner_type,
        &request_key.topic.owner_id,
        &request_key.topic.topic_id,
        Some(&json!({"expectedGeneration": expected_generation})),
        &endpoint.token,
    )?;
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
    tokio::time::timeout(HELPER_COMMAND_TIMEOUT, async {
        stream
            .write_all(&len.to_be_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream
            .write_all(cmd_bytes)
            .await
            .map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "写入 helper stop 命令超时".to_string())??;
    Ok(())
}

/// Reserve the Kotlin-side same-generation handoff before Rust aborts its old
/// reader.  The response is an ordering barrier: after it is observed, an old
/// socket EOF cannot remove the helper session while resume ACK/replay is in
/// flight.
#[cfg(target_os = "android")]
pub(crate) async fn prepare_resume_with_helper<R: Runtime>(
    app: &AppHandle<R>,
    request_key: &MessageKey,
    expected_generation: u64,
) -> Result<(), String> {
    let stream = connect_to_helper(
        app,
        "prepare_resume",
        request_key,
        Some(json!({"expectedGeneration": expected_generation})),
    )
    .await?;
    let mut reader = FramedRead::new(stream, LengthDelimitedCodec::new());
    let frame = tokio::time::timeout(HELPER_COMMAND_TIMEOUT, reader.next())
        .await
        .map_err(|_| "helper 接管准备响应超时".to_string())?
        .ok_or_else(|| "helper 接管准备连接提前关闭".to_string())?
        .map_err(|error| format!("读取 helper 接管准备响应失败: {error}"))?;
    let response: Value = serde_json::from_slice(&frame)
        .map_err(|error| format!("解析 helper 接管准备响应失败: {error}"))?;
    validate_resume_takeover_ack(request_key, expected_generation, &response)
}

/// Best-effort cancellation for a prepared handoff whose candidate connection
/// never reached Kotlin resume.  The bounded Kotlin lease remains the final
/// safety net if this command cannot be delivered.
#[cfg(target_os = "android")]
pub(crate) async fn cancel_resume_with_helper<R: Runtime>(
    app: &AppHandle<R>,
    request_key: &MessageKey,
    expected_generation: u64,
) -> Result<(), String> {
    connect_to_helper(
        app,
        "cancel_resume",
        request_key,
        Some(json!({"expectedGeneration": expected_generation})),
    )
    .await
    .map(|_| ())
}

#[cfg(target_os = "android")]
fn validate_resume_takeover_ack(
    request_key: &MessageKey,
    expected_generation: u64,
    response: &Value,
) -> Result<(), String> {
    for (field, expected) in [
        ("requestId", request_key.msg_id.as_str()),
        ("messageId", request_key.msg_id.as_str()),
        ("ownerType", request_key.topic.owner_type.as_str()),
        ("ownerId", request_key.topic.owner_id.as_str()),
        ("topicId", request_key.topic.topic_id.as_str()),
    ] {
        if response[field].as_str() != Some(expected) {
            return Err(format!("helper 接管准备响应身份字段 {field} 不一致"));
        }
    }
    if response["eventType"].as_str() != Some("resume_pending") {
        return Err("helper 接管准备响应类型无效".to_string());
    }
    if response["generation"].as_u64() != Some(expected_generation) {
        return Err("helper 接管准备响应 generation 不匹配".to_string());
    }
    Ok(())
}

#[cfg(test)]
#[path = "vcp_client_transport_tests.rs"]
mod tests;
