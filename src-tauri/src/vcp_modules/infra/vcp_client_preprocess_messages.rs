use crate::vcp_modules::media_processor::convert_local_image_for_multimodal;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

pub(crate) async fn preprocess_multimodal_messages<R: Runtime>(
    app: &AppHandle<R>,
    raw_messages: Vec<Value>,
) -> Result<Vec<Value>, String> {
    let mut messages = Vec::with_capacity(raw_messages.len());
    for message in raw_messages {
        if !message.is_object() {
            messages.push(json!({"role": "system", "content": "[Invalid message]"}));
        } else {
            messages.push(preprocess_message(app, message).await);
        }
    }
    Ok(messages)
}

async fn preprocess_message<R: Runtime>(app: &AppHandle<R>, value: Value) -> Value {
    let mut message = value;
    let content = message.get("content").cloned().unwrap_or(Value::Null);
    if let Some(parts) = content.as_array() {
        message["content"] = json!(preprocess_content_parts(app, parts).await);
    } else if let Some(object) = content.as_object() {
        message["content"] = object
            .get("text")
            .cloned()
            .unwrap_or_else(|| json!(content.to_string()));
    } else if !content.is_string() && !content.is_null() {
        message["content"] = json!(content.to_string());
    }
    message
}

async fn preprocess_content_parts<R: Runtime>(app: &AppHandle<R>, parts: &[Value]) -> Vec<Value> {
    let mut converted_parts = Vec::new();
    for part in parts {
        converted_parts.extend(preprocess_content_part(app, part).await);
    }
    converted_parts
}

async fn preprocess_content_part<R: Runtime>(app: &AppHandle<R>, part: &Value) -> Vec<Value> {
    let Some(object) = part.as_object() else {
        return vec![part.clone()];
    };
    if object.get("type").and_then(Value::as_str) != Some("local_file") {
        return vec![part.clone()];
    }
    let Some(path) = object.get("path").and_then(Value::as_str) else {
        return Vec::new();
    };
    convert_local_file(app, path).await
}

async fn convert_local_file<R: Runtime>(app: &AppHandle<R>, path: &str) -> Vec<Value> {
    let clean_path = path.replace("file://", "");
    let path_buf = PathBuf::from(&clean_path);
    let (mime, part_type) = classify_file(&path_buf);
    let converted = if path_buf.exists() {
        convert_existing_file(app, &path_buf, mime, part_type).await
    } else {
        None
    };
    converted.unwrap_or_else(|| vec![fallback_file_part(&clean_path, mime)])
}

fn classify_file(path: &Path) -> (&'static str, &'static str) {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "heic" | "heif" | "avif" => {
            ("image", "image_url")
        }
        "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "opus" | "amr" => ("audio", "input_audio"),
        "mp4" | "webm" | "3gp" | "3g2" | "mov" => ("video", "image_url"),
        _ => ("application", "file_url"),
    }
}

async fn convert_existing_file<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    mime: &str,
    part_type: &str,
) -> Option<Vec<Value>> {
    match mime {
        "image" => convert_image(app, path, part_type).await,
        "video" => convert_video(app, path).await,
        "audio" => convert_audio(app, path).await,
        _ => None,
    }
}

async fn convert_image<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    part_type: &str,
) -> Option<Vec<Value>> {
    let path = path.to_path_buf();
    let conversion_path = path.clone();
    match tokio::task::spawn_blocking({
        let app = app.clone();
        move || convert_local_image_for_multimodal(&app, &conversion_path)
    })
    .await
    {
        Ok(Ok(data_url)) => Some(vec![json!({
            "type": part_type,
            part_type: { "url": data_url }
        })]),
        Ok(Err(error)) => {
            log::warn!(
                "[VCPClient] Image conversion failed for {:?}: {}",
                path,
                error
            );
            None
        }
        Err(error) => {
            log::warn!("[VCPClient] Image conversion task panicked: {}", error);
            None
        }
    }
}

async fn convert_video<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Option<Vec<Value>> {
    let path = path.to_path_buf();
    let conversion_path = path.clone();
    match tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            crate::vcp_modules::media_processor::process_video_for_multimodal(
                &app,
                &conversion_path,
            )
        }
    })
    .await
    {
        Ok(Ok(frames)) => Some(
            frames
                .into_iter()
                .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                .collect(),
        ),
        Ok(Err(error)) => {
            log::warn!(
                "[VCPClient] Video frame extraction failed for {:?}: {}",
                path,
                error
            );
            None
        }
        Err(error) => {
            log::warn!("[VCPClient] Video processing task panicked: {}", error);
            None
        }
    }
}

async fn convert_audio<R: Runtime>(app: &AppHandle<R>, path: &Path) -> Option<Vec<Value>> {
    let path = path.to_path_buf();
    let conversion_path = path.clone();
    match tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            crate::vcp_modules::media_processor::process_audio_for_multimodal(
                &app,
                &conversion_path,
            )
        }
    })
    .await
    {
        Ok(Ok(audio_url)) => {
            let format = if audio_url.starts_with("data:audio/aac") {
                "aac"
            } else {
                "mp3"
            };
            Some(vec![json!({
                "type": "input_audio",
                "input_audio": { "data": audio_url, "format": format }
            })])
        }
        Ok(Err(error)) => {
            log::warn!(
                "[VCPClient] Audio extraction failed for {:?}: {}",
                path,
                error
            );
            None
        }
        Err(error) => {
            log::warn!("[VCPClient] Audio processing task panicked: {}", error);
            None
        }
    }
}

fn fallback_file_part(path: &str, mime: &str) -> Value {
    let text = if mime == "image" {
        format!(
            "[附件文件: {path}]\n<system_meta>[系统提示]：由于硬件环境限制或原图过大，该图片的视觉信息提取失败，已转为纯文本占位符，请提醒用户注意。</system_meta>"
        )
    } else {
        format!("[附件文件: {path}]")
    };
    json!({"type": "text", "text": text})
}
