use reqwest::header::AUTHORIZATION;
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

/// 测试 VCP 后端连接状态并获取模型列表 (对齐桌面端 main.js fetchAndCacheModels 逻辑)
#[tauri::command]
pub async fn test_vcp_connection(vcp_url: String, vcp_api_key: String) -> Result<Value, String> {
    log::info!(
        "[VCPClient] test_vcp_connection called for URL: {}",
        vcp_url
    );

    let models_url = build_models_url(&vcp_url)?;

    log::info!(
        "[VCPClient] Testing connection to (Original Logic): {}",
        models_url
    );

    let client = Client::builder()
        .timeout(Duration::from_secs(10)) // 测试连接 10s 超时即可
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(&models_url)
        .header(AUTHORIZATION, format!("Bearer {}", vcp_api_key))
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    parse_models_response(res).await
}

fn build_models_url(vcp_url: &str) -> Result<String, String> {
    let url = Url::parse(vcp_url).map_err(|error| format!("URL 解析失败: {error}"))?;
    let port = url
        .port()
        .map(|value| format!(":{value}"))
        .unwrap_or_default();
    let base_url = format!(
        "{}://{}{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        port
    );
    Ok(if base_url.ends_with('/') {
        format!("{base_url}v1/models")
    } else {
        format!("{base_url}/v1/models")
    })
}

async fn parse_models_response(res: reqwest::Response) -> Result<Value, String> {
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!("验证失败 ({}): {}", status.as_u16(), text));
    }
    let models: Value = res
        .json()
        .await
        .map_err(|error| format!("JSON解析失败: {error}"))?;
    let model_count = models
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    Ok(json!({
        "success": true,
        "status": status.as_u16(),
        "modelCount": model_count,
        "models": models
    }))
}
