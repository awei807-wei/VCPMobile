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
    let declared_mime = object.get("mime").and_then(Value::as_str);
    let declared_name = object.get("name").and_then(Value::as_str);
    convert_local_file(app, path, declared_mime, declared_name).await
}

async fn convert_local_file<R: Runtime>(
    app: &AppHandle<R>,
    path: &str,
    declared_mime: Option<&str>,
    declared_name: Option<&str>,
) -> Vec<Value> {
    let clean_path = path.strip_prefix("file://").unwrap_or(path);
    let path_buf = PathBuf::from(clean_path);
    let safe_name = safe_attachment_name(declared_name, clean_path);
    let (media_kind, part_type) = classify_file(&path_buf, declared_mime);
    let converted = if path_buf.exists() {
        convert_existing_file(app, &path_buf, media_kind, part_type).await
    } else {
        None
    };
    converted.unwrap_or_else(|| vec![fallback_file_part(&safe_name, media_kind)])
}

fn classify_file(path: &Path, declared_mime: Option<&str>) -> (&'static str, &'static str) {
    if let Some(declared_mime) = declared_mime {
        let normalized_mime = declared_mime
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if normalized_mime == "image" || normalized_mime.starts_with("image/") {
            return ("image", "image_url");
        }
        if normalized_mime == "audio" || normalized_mime.starts_with("audio/") {
            return ("audio", "input_audio");
        }
        if normalized_mime == "video" || normalized_mime.starts_with("video/") {
            return ("video", "image_url");
        }
    }

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

fn sanitized_file_name(candidate: &str) -> Option<String> {
    let base_name = candidate
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or(candidate);
    let cleaned = base_name
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || matches!(cleaned, "." | "..") {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn safe_attachment_name(declared_name: Option<&str>, path: &str) -> String {
    declared_name
        .and_then(sanitized_file_name)
        .or_else(|| sanitized_file_name(path))
        .unwrap_or_else(|| "附件".to_string())
}

async fn convert_existing_file<R: Runtime>(
    app: &AppHandle<R>,
    path: &Path,
    media_kind: &str,
    part_type: &str,
) -> Option<Vec<Value>> {
    match media_kind {
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

fn fallback_file_part(file_name: &str, media_kind: &str) -> Value {
    let text = if media_kind == "image" {
        format!(
            "[附件文件: {file_name}]\n<system_meta>[系统提示]：由于硬件环境限制或原图过大，该图片的视觉信息提取失败，已转为纯文本占位符，请提醒用户注意。</system_meta>"
        )
    } else {
        format!("[附件文件: {file_name}]")
    };
    json!({"type": "text", "text": text})
}

#[cfg(test)]
mod tests {
    use super::{classify_file, fallback_file_part, safe_attachment_name};
    use std::path::Path;

    #[test]
    fn classify_file_prefers_declared_image_mime_without_extension() {
        assert_eq!(
            classify_file(Path::new("/tmp/no-extension"), Some("image/png")),
            ("image", "image_url")
        );
        assert_eq!(
            classify_file(Path::new("/tmp/no-extension"), Some("image")),
            ("image", "image_url")
        );
    }

    #[test]
    fn classify_file_accepts_mime_parameters() {
        assert_eq!(
            classify_file(
                Path::new("/tmp/no-extension"),
                Some(" IMAGE/PNG ; charset=binary ")
            ),
            ("image", "image_url")
        );
    }

    #[test]
    fn classify_file_falls_back_to_known_extensions() {
        assert_eq!(
            classify_file(
                Path::new("/tmp/photo.png"),
                Some("application/octet-stream")
            ),
            ("image", "image_url")
        );
        assert_eq!(
            classify_file(Path::new("/tmp/sound.mp3"), None),
            ("audio", "input_audio")
        );
        assert_eq!(
            classify_file(Path::new("/tmp/movie.mp4"), None),
            ("video", "image_url")
        );
    }

    #[test]
    fn classify_file_prefers_mime_over_conflicting_extension() {
        assert_eq!(
            classify_file(Path::new("/tmp/not-really-audio.mp3"), Some("image/jpeg")),
            ("image", "image_url")
        );
    }

    #[test]
    fn safe_attachment_name_handles_posix_and_windows_paths() {
        assert_eq!(
            safe_attachment_name(None, "/private/app/cache/photo.png"),
            "photo.png"
        );
        assert_eq!(
            safe_attachment_name(Some(r"C:\Users\Alice\Pictures\holiday.jpg"), "/ignored"),
            "holiday.jpg"
        );
    }

    #[test]
    fn image_fallback_uses_only_safe_name_and_keeps_failure_notice() {
        let posix_name = safe_attachment_name(None, "/private/app/cache/photo.png");
        let posix_part = fallback_file_part(&posix_name, "image");
        let posix_text = posix_part["text"].as_str().expect("fallback text");
        assert!(posix_text.contains("photo.png"));
        assert!(!posix_text.contains("/private/app/cache"));
        assert!(posix_text.contains("该图片的视觉信息提取失败"));

        let windows_name = safe_attachment_name(None, r"C:\Users\Alice\Pictures\holiday.jpg");
        let windows_part = fallback_file_part(&windows_name, "image");
        let windows_text = windows_part["text"].as_str().expect("fallback text");
        assert!(windows_text.contains("holiday.jpg"));
        assert!(!windows_text.contains(r"C:\Users\Alice"));
        assert!(windows_text.contains("该图片的视觉信息提取失败"));
    }
}
