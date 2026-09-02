use tauri::{AppHandle, Manager};

use super::super::{
    get_attachments_root_dir, get_multimodal_cache_dir, get_thumbnails_root_dir,
    safe_storage_extension,
};

/// 内部辅助函数：智能启发式检测文件是否可能为纯文本
/// 读取前 1024 字节，如果不包含 NULL 字节 (0x00)，则极大概率是文本或代码
fn is_likely_text_file(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut buffer = [0u8; 1024];
    let n = match file.read(&mut buffer) {
        Ok(n) => n,
        Err(_) => return false,
    };

    if n == 0 {
        return false;
    }

    // 检查已读取的部分是否含有 NULL 字节
    for &b in &buffer[..n] {
        if b == 0 {
            return false;
        }
    }
    true
}

/// 内部辅助函数：精细化 MIME 类型判定 (对齐桌面端 fileManager.js)
/// 增加了魔数检测 (infer) 和 文本启发式检测 (no-NULL sniffing)
pub fn get_refined_mime_type(
    path: &std::path::Path,
    original_name: &str,
    initial_mime: &str,
) -> String {
    let ext = std::path::Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // 1. 强制修正 MP3
    if ext == "mp3" {
        return "audio/mpeg".to_string();
    }

    // 2. 如果初始值无效，或者是一个通用后缀，则尝试根据扩展名路由
    let current_mime = initial_mime.to_string();

    if current_mime.is_empty() || current_mime == "application/octet-stream" {
        match ext.as_str() {
            "txt" => return "text/plain".to_string(),
            "json" => return "application/json".to_string(),
            "xml" => return "application/xml".to_string(),
            "csv" => return "text/csv".to_string(),
            "html" => return "text/html".to_string(),
            "css" => return "text/css".to_string(),
            "pdf" => return "application/pdf".to_string(),
            "doc" => return "application/msword".to_string(),
            "docx" => {
                return "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_string()
            }
            "xls" => return "application/vnd.ms-excel".to_string(),
            "xlsx" => {
                return "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    .to_string()
            }
            "ppt" => return "application/vnd.ms-powerpoint".to_string(),
            "pptx" => {
                return "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                    .to_string()
            }
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "png" => return "image/png".to_string(),
            "gif" => return "image/gif".to_string(),
            "webp" => return "image/webp".to_string(),
            "svg" => return "image/svg+xml".to_string(),
            "bmp" => return "image/bmp".to_string(),
            "ico" => return "image/x-icon".to_string(),
            "tiff" | "tif" => return "image/tiff".to_string(),
            "heic" | "heif" => return "image/heic".to_string(),
            "avif" => return "image/avif".to_string(),
            "wav" => return "audio/wav".to_string(),
            "ogg" | "ogv" => return "audio/ogg".to_string(),
            "flac" => return "audio/flac".to_string(),
            "aac" => return "audio/aac".to_string(),
            "aiff" | "aif" => return "audio/aiff".to_string(),
            "m4a" => return "audio/mp4".to_string(),
            "opus" => return "audio/opus".to_string(),
            "amr" => return "audio/amr".to_string(),
            "mp4" | "m4v" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "mov" | "qt" => return "video/quicktime".to_string(),
            "avi" => return "video/x-msvideo".to_string(),
            "mkv" => return "video/x-matroska".to_string(),
            "wmv" => return "video/x-ms-wmv".to_string(),
            "flv" => return "video/x-flv".to_string(),
            "3gp" | "3g2" => return "video/3gpp".to_string(),
            "mts" | "m2ts" => return "video/mp2t".to_string(),
            // 所有代码/文本类文件统一为 text/plain 以触发提取逻辑
            _ if crate::vcp_modules::infra::file_extractor::is_text_or_code_extension(&ext) => {
                return "text/plain".to_string();
            }
            _ => {
                // 3. 终极兜底：物理层嗅探
                if path.exists() {
                    // 3a. 魔数匹配 (用于识别被改了后缀的二进制文件)
                    if let Ok(Some(kind)) = infer::get_from_path(path) {
                        return kind.mime_type().to_string();
                    }

                    // 3b. 文本启发式 (用于识别未知的文本/代码格式，如 .pub, .env, .log)
                    if is_likely_text_file(path) {
                        return "text/plain".to_string();
                    }
                }
            }
        }
    }

    current_mime
}

/// 内部辅助函数：生成图片缩略图（短边 200px 自适应，已下沉到 Android Kotlin 侧，此处直接返回 None）
pub async fn generate_thumbnail<R: tauri::Runtime>(
    _app_handle: &tauri::AppHandle<R>,
    _original_path: &std::path::Path,
    _hash: &str,
) -> Option<String> {
    None
}

/// 内部辅助函数：校验路径安全性，防止路径遍历攻击
pub(super) fn ensure_safe_path(
    app_handle: &AppHandle,
    path: &std::path::Path,
) -> Result<(), String> {
    // 物理展开目标路径的所有相对路径分量 (..)，杜绝字符级前缀欺骗的沙盒逃逸
    let canonical_path = if path.exists() {
        std::fs::canonicalize(path).map_err(|e| format!("路径规范化失败: {}", e))?
    } else {
        // 如果文件甚至不存在，安全起见直接阻断，因为在 register_local_file 中已校验 exists()，
        // open_file 也同样应阻断不存在的文件访问以防信息探测
        return Err("非法路径访问：目标文件不存在".to_string());
    };

    let config_dir = app_handle
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    let canonical_config = std::fs::canonicalize(&config_dir).unwrap_or(config_dir);

    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?;
    let canonical_cache = std::fs::canonicalize(&cache_dir).unwrap_or(cache_dir);

    // 允许访问 App 配置目录 (内部)、缓存目录 (临时)、附件目录 (可能在外部) 或 缩略图目录
    let attachments_dir = get_attachments_root_dir(app_handle)?;
    let canonical_attachments = std::fs::canonicalize(&attachments_dir).unwrap_or(attachments_dir);

    let thumbnails_dir = get_thumbnails_root_dir(app_handle)?;
    let canonical_thumbnails = std::fs::canonicalize(&thumbnails_dir).unwrap_or(thumbnails_dir);

    let multimodal_cache_dir = get_multimodal_cache_dir(app_handle)?;
    let canonical_multimodal_cache =
        std::fs::canonicalize(&multimodal_cache_dir).unwrap_or(multimodal_cache_dir);

    if canonical_path.starts_with(&canonical_config)
        || canonical_path.starts_with(&canonical_cache)
        || canonical_path.starts_with(&canonical_attachments)
        || canonical_path.starts_with(&canonical_thumbnails)
        || canonical_path.starts_with(&canonical_multimodal_cache)
    {
        Ok(())
    } else {
        Err(format!(
            "非法路径访问：禁止访问应用授权范围以外的文件 ({:?})",
            path
        ))
    }
}

/// 内部辅助函数：获取当前平台下的真实路径 (用于历史记录自动纠错)
#[allow(dead_code)]
pub fn resolve_attachment_path(
    app_handle: &AppHandle,
    hash: &str,
    original_name: &str,
) -> Option<String> {
    if !crate::vcp_modules::infra::utils::is_valid_cas_hash(hash) {
        return None;
    }
    let attachments_dir = get_attachments_root_dir(app_handle).ok()?;

    let internal_file_name = safe_storage_extension(original_name)
        .map(|extension| format!("{}.{}", hash, extension))
        .unwrap_or_else(|| hash.to_string());

    let full_path = attachments_dir.join(internal_file_name);
    if full_path.exists() {
        Some(full_path.to_string_lossy().to_string())
    } else {
        None
    }
}
