#[cfg(target_os = "android")]
use crate::vcp_modules::chat::topic_types::MessageKey;
#[cfg(target_os = "android")]
use serde_json::{json, Value};
#[cfg(target_os = "android")]
use std::time::Duration;
#[cfg(target_os = "android")]
use tauri::{AppHandle, Manager, Runtime};

#[cfg(target_os = "android")]
pub(crate) fn get_helper_port<R: Runtime>(app: &AppHandle<R>) -> Result<u16, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");
    if !port_file.exists() {
        return Err("sse_helper.port file not found. Is SseProxyService running?".to_string());
    }
    let content = std::fs::read_to_string(port_file).map_err(|e| e.to_string())?;
    let port = content.trim().parse::<u16>().map_err(|e| e.to_string())?;
    Ok(port)
}

#[cfg(target_os = "android")]
pub(crate) async fn connect_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    action: &str,
    msg_id: &str,
    extra_params: Option<Value>,
) -> Result<tokio::net::TcpStream, String> {
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let port_file = cache_dir.join("sse_helper.port");

    // 1. 尝试使用已有的端口文件进行连接（适用于 helper 已经在运行且就绪的情况）
    if let Some(stream) = connect_existing_helper(&port_file).await {
        log::info!("[VCPClient] Connected to existing sse helper socket");
        return send_command_to_stream(stream, action, msg_id, extra_params).await;
    }

    // 2. 如果连接失败或文件不存在，启动/唤醒 helper 服务
    log::info!(
        "[VCPClient] Helper not responding or port file missing. Starting/Waking helper service..."
    );
    let _ = tauri_plugin_vcp_mobile::stream::start_helper_service(app.clone());

    // 3. 循环等待新端口文件并尝试连接（最多尝试 60 次，每次间隔 50ms，总计 3 秒超时）
    let stream = wait_for_helper(&port_file).await?;
    send_command_to_stream(stream, action, msg_id, extra_params).await
}

#[cfg(target_os = "android")]
async fn connect_existing_helper(port_file: &std::path::Path) -> Option<tokio::net::TcpStream> {
    if !port_file.exists() {
        return None;
    }
    let content = std::fs::read_to_string(port_file).ok()?;
    let port = content.trim().parse::<u16>().ok()?;
    tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .ok()
}

#[cfg(target_os = "android")]
async fn wait_for_helper(port_file: &std::path::Path) -> Result<tokio::net::TcpStream, String> {
    let max_attempts = 60;
    let delay = Duration::from_millis(50);
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let Some(port) = read_helper_port_for_retry(port_file, &mut last_err) else {
            tokio::time::sleep(delay).await;
            continue;
        };
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(stream) => {
                log::info!(
                    "[VCPClient] Connected to sse helper socket on 127.0.0.1:{} after {} attempts",
                    port,
                    attempt
                );
                return Ok(stream);
            }
            Err(error) => {
                last_err = format!("Connect to 127.0.0.1:{port} failed: {error}");
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
fn read_helper_port_for_retry(path: &std::path::Path, last_err: &mut String) -> Option<u16> {
    if !path.exists() {
        return None;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            *last_err = format!("Read port file error: {error}");
            return None;
        }
    };
    let port = content.trim();
    if port.is_empty() {
        return None;
    }
    match port.parse::<u16>() {
        Ok(port) => Some(port),
        Err(error) => {
            *last_err = format!("Parse port error: {error}");
            None
        }
    }
}

// 辅助函数：向已连接的 TcpStream 发送 JSON 指令
#[cfg(target_os = "android")]
pub(crate) async fn send_command_to_stream(
    mut stream: tokio::net::TcpStream,
    action: &str,
    msg_id: &str,
    extra_params: Option<Value>,
) -> Result<tokio::net::TcpStream, String> {
    let mut cmd = json!({
        "action": action,
        "requestId": msg_id
    });
    if let Some(params) = extra_params {
        if let Some(obj) = cmd.as_object_mut() {
            for (k, v) in params.as_object().unwrap() {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    use tokio::io::AsyncWriteExt;
    let cmd_str = cmd.to_string();
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
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
    Ok(stream)
}

#[cfg(target_os = "android")]
pub(crate) async fn send_stop_to_helper<R: Runtime>(
    app: &AppHandle<R>,
    msg_id: &str,
    request_key: &MessageKey,
) -> Result<(), String> {
    let port = get_helper_port(app)?;
    let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .map_err(|e| e.to_string())?;

    let cmd = json!({
        "action": "stop",
        "requestId": msg_id,
        "ownerType": request_key.topic.owner_type,
        "ownerId": request_key.topic.owner_id,
        "topicId": request_key.topic.topic_id
    });

    use tokio::io::AsyncWriteExt;
    let cmd_str = cmd.to_string();
    let cmd_bytes = cmd_str.as_bytes();
    let len = cmd_bytes.len() as u32;
    stream
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream
        .write_all(cmd_bytes)
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}
