use crate::vcp_modules::sync_logger::redact_sync_diagnostic;
use std::time::Duration;
use tokio_tungstenite::tungstenite::error::Error as WsError;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionErrorDiagnosis {
    pub error_code: String,
    pub error_message: String,
    pub solution: String,
    pub error_detail: String,
}

pub(crate) fn check_loopback_on_mobile(ws_url: &str, is_android: bool) -> bool {
    if !is_android {
        return false;
    }
    url::Url::parse(ws_url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host == "127.0.0.1" || host == "localhost")
        })
        .unwrap_or(false)
}

pub(crate) async fn diagnose_connection_failure(
    ws_url: &str,
    http_url: &str,
    err: &WsError,
) -> ConnectionErrorDiagnosis {
    let detail = err.to_string();
    if let Some(diagnosis) = diagnose_io(err, &detail) {
        return diagnosis;
    }
    if let Some(diagnosis) = diagnose_mobile_loopback(ws_url, &detail) {
        return diagnosis;
    }
    if let Some(diagnosis) = diagnose_http_status(err, &detail) {
        return diagnosis;
    }
    diagnose_http_probe(http_url, &detail).await
}

fn diagnose_io(err: &WsError, detail: &str) -> Option<ConnectionErrorDiagnosis> {
    let WsError::Io(io_error) = err else {
        return None;
    };
    if !matches!(
        io_error.kind(),
        std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::NetworkUnreachable
            | std::io::ErrorKind::HostUnreachable
    ) {
        return None;
    }
    Some(ConnectionErrorDiagnosis {
        error_code: "NETWORK_UNREACHABLE".to_string(),
        error_message: "网络不可达或地址无效".to_string(),
        solution: "无法建立连接。请检查手机网络状态，确保 WiFi 已连接且配置了正确的电脑端局域网 IP 和端口。".to_string(),
        error_detail: detail.to_string(),
    })
}

fn diagnose_mobile_loopback(ws_url: &str, detail: &str) -> Option<ConnectionErrorDiagnosis> {
    if !check_loopback_on_mobile(ws_url, cfg!(target_os = "android")) {
        return None;
    }
    Some(ConnectionErrorDiagnosis {
        error_code: "CONFIG_LOOPBACK_ON_MOBILE".to_string(),
        error_message: "移动端配置了本地回环地址".to_string(),
        solution: "移动设备无法通过 127.0.0.1 或 localhost 访问电脑端的服务。请确认电脑与手机连接在同一个 WiFi 下，并在移动端设置中将同步 IP 改为电脑的局域网 IP。".to_string(),
        error_detail: detail.to_string(),
    })
}

fn diagnose_http_status(err: &WsError, detail: &str) -> Option<ConnectionErrorDiagnosis> {
    let WsError::Http(response) = err else {
        return None;
    };
    let status = response.status();
    let (error_code, error_message, solution) = match status.as_u16() {
        401 | 403 => (
            "TOKEN_MISMATCH",
            "身份认证失败（Token 错误）",
            "移动端设置的同步令牌与桌面端不匹配。请检查两端配置。",
        ),
        404 => (
            "WS_PATH_INVALID",
            "同步服务路径不存在 (404)",
            "请确保桌面端同步插件已启用并检查同步 IP 和端口配置。",
        ),
        _ => (
            "HTTP_HANDSHAKE_REJECTED",
            "握手被服务器拒绝",
            "请检查服务器状态、端口配置或桌面端同步插件日志。",
        ),
    };
    Some(ConnectionErrorDiagnosis {
        error_code: error_code.to_string(),
        error_message: if status.as_u16() >= 400 && status.as_u16() != 401 && status.as_u16() != 403
        {
            format!("{} (HTTP {})", error_message, status.as_u16())
        } else {
            error_message.to_string()
        },
        solution: solution.to_string(),
        error_detail: detail.to_string(),
    })
}

async fn diagnose_http_probe(http_url: &str, detail: &str) -> ConnectionErrorDiagnosis {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();
    match client.get(http_url).send().await {
        Ok(response) => diagnose_probe_response(response.status(), detail),
        Err(error) => diagnose_probe_error(error, detail),
    }
}

fn diagnose_probe_response(status: reqwest::StatusCode, detail: &str) -> ConnectionErrorDiagnosis {
    if status.is_success() || matches!(status.as_u16(), 401 | 403) {
        return ConnectionErrorDiagnosis {
            error_code: "WS_UPGRADE_FAILED".to_string(),
            error_message: "HTTP 访问正常，但 WebSocket 升级失败".to_string(),
            solution: "请检查桌面端同步插件或代理软件是否拦截了 WebSocket。".to_string(),
            error_detail: format!("HTTP Status: {status}, WS Err: {detail}"),
        };
    }
    ConnectionErrorDiagnosis {
        error_code: "HTTP_PROBE_ERROR".to_string(),
        error_message: format!("HTTP 探测返回异常 (HTTP {})", status.as_u16()),
        solution: "请确认桌面端服务运行正常并加载了正确的同步插件。".to_string(),
        error_detail: format!("HTTP Status: {status}, WS Err: {detail}"),
    }
}

fn diagnose_probe_error(error: reqwest::Error, detail: &str) -> ConnectionErrorDiagnosis {
    let (error_code, error_message, solution) = if error.is_timeout() {
        (
            "NETWORK_TIMEOUT",
            "连接超时，无法访问服务器",
            "请检查手机与电脑是否在同一 WiFi、电脑防火墙及同步 IP。",
        )
    } else if error.is_connect() {
        (
            "CONNECTION_REFUSED",
            "连接被拒绝 (Connection Refused)",
            "请确保桌面端已启动且同步服务端口配置正确。",
        )
    } else {
        (
            "NETWORK_UNREACHABLE",
            "网络不可达或地址无效",
            "请检查手机网络状态及电脑端局域网 IP。",
        )
    };
    ConnectionErrorDiagnosis {
        error_code: error_code.to_string(),
        error_message: error_message.to_string(),
        solution: solution.to_string(),
        error_detail: redact_sync_diagnostic(&format!(
            "WS Err: {detail} | HTTP Probe Err: {error}"
        )),
    }
}
