use std::fs;
use tauri::{AppHandle, Manager};

use super::super::{get_multimodal_cache_dir, get_thumbnails_root_dir};
use super::preview::ensure_safe_path;

/// 唤起系统默认应用打开文件或 URL
#[tauri::command]
pub async fn open_file(app_handle: AppHandle, path: String) -> Result<(), String> {
    let clean_path = path.replace("file://", "");

    // 网络 URL 直接打开，跳过本地路径安全校验
    if clean_path.starts_with("http://") || clean_path.starts_with("https://") {
        use tauri_plugin_opener::OpenerExt;
        return app_handle
            .opener()
            .open_url(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string());
    }

    let path_buf = std::path::PathBuf::from(&clean_path);

    // 安全校验：禁止打开系统敏感路径
    ensure_safe_path(&app_handle, &path_buf)?;

    #[cfg(target_os = "android")]
    {
        return tauri_plugin_vcp_mobile::system::open_file_native(app_handle, clean_path);
    }

    // 使用 tauri-plugin-opener 的原生能力
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt;
        app_handle
            .opener()
            .open_path(clean_path, Option::<String>::None)
            .map_err(|e| e.to_string())
    }
}

/// 清理上传缓存目录以及多媒体缓存碎片 (通常在启动时执行，清除上次闪退/强杀留下的僵尸文件)
pub fn clear_upload_cache(app_handle: &AppHandle) {
    if let Ok(cache_dir) = app_handle.path().app_cache_dir() {
        // 1. 清除上次上传未完成的分片临时目录
        let uploads_path = cache_dir.join("uploads");
        if uploads_path.exists() {
            let _ = fs::remove_dir_all(&uploads_path);
            let _ = fs::create_dir_all(&uploads_path);
            log::info!("[FileManager] Upload cache cleared.");
        }

        // 2. 🌟多媒体零拷贝转码垃圾碎片冷启动清理防线（Sweeper）🌟
        // 遍历整个 cache_dir，只要文件/目录名匹配 img_*.webp、aud_*.aac 或以 vid_ 开头的一律物理抹除，彻底防止碎屑泄露
        if cache_dir.exists() && cache_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&cache_dir) {
                let mut cleared_files = 0;
                let mut cleared_dirs = 0;
                for entry in entries.flatten() {
                    let path = entry.path();
                    let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                    if (file_name.starts_with("img_") && file_name.ends_with(".webp"))
                        || (file_name.starts_with("aud_") && file_name.ends_with(".aac"))
                        || (file_name.starts_with("camera_") && file_name.ends_with(".jpg"))
                        || (file_name.starts_with("pick_") && file_name.ends_with("_temp"))
                    {
                        if fs::remove_file(&path).is_ok() {
                            cleared_files += 1;
                        }
                    } else if file_name.starts_with("vid_")
                        && path.is_dir()
                        && fs::remove_dir_all(&path).is_ok()
                    {
                        cleared_dirs += 1;
                    }
                }
                if cleared_files > 0 || cleared_dirs > 0 {
                    log::info!(
                        "[FileManager] Cold-boot GC: Swept {} media cache files and {} zombie video folders.",
                        cleared_files,
                        cleared_dirs
                    );
                }
            }
        }

        // 3. 自动收敛多模态结果缓存 (300MB 阈值，收敛至 150MB)
        evict_multimodal_cache_if_needed(app_handle, 300 * 1024 * 1024, 150 * 1024 * 1024);
    }
}

/// 限制并收敛多模态结果缓存目录的总大小 (LRU 思想，基于 mtime 淘汰最旧的 json 缓存)
/// 当总大小超过 max_size_bytes (e.g. 300MB) 时，自动淘汰最旧的缓存，直到大小收缩到 target_size_bytes (e.g. 150MB)
pub fn evict_multimodal_cache_if_needed<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    max_size_bytes: u64,
    target_size_bytes: u64,
) {
    let cache_dir = match get_multimodal_cache_dir(app_handle) {
        Ok(dir) => dir,
        Err(_) => return,
    };

    if !cache_dir.exists() || !cache_dir.is_dir() {
        return;
    }

    let entries = match fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    struct CacheFile {
        path: std::path::PathBuf,
        size: u64,
        mtime: std::time::SystemTime,
    }

    let mut cache_files = Vec::new();
    let mut total_size = 0u64;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Ok(meta) = fs::metadata(&path) {
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .unwrap_or_else(|_| std::time::SystemTime::now());
                total_size += size;
                cache_files.push(CacheFile { path, size, mtime });
            }
        }
    }

    if total_size <= max_size_bytes {
        return;
    }

    log::info!(
        "[FileManager] Multimodal cache size ({} MB) exceeds limit ({} MB). Starting eviction...",
        total_size / 1024 / 1024,
        max_size_bytes / 1024 / 1024
    );

    // 按照修改时间升序排列 (最旧的在前面)
    cache_files.sort_by_key(|f| f.mtime);

    let mut evicted_count = 0;
    let mut evicted_size = 0u64;

    for file in cache_files {
        if total_size - evicted_size <= target_size_bytes {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            evicted_size += file.size;
            evicted_count += 1;
        }
    }

    log::info!(
        "[FileManager] Multimodal cache eviction complete. Evicted {} files, freed {:.2} MB. Current size: {:.2} MB.",
        evicted_count,
        evicted_size as f64 / 1024.0 / 1024.0,
        (total_size - evicted_size) as f64 / 1024.0 / 1024.0
    );
}

/// ⚡ 确保附件大文本已被安全提取。
/// 若数据库中缺失大文本，且手机本地物理文件真实存在，则在后台立即触发提取，并异步持久化自愈回库。
pub async fn ensure_extracted_text(
    pool: &sqlx::SqlitePool,
    hash: &str,
    internal_path: &str,
    mime_type: &str,
) -> Option<String> {
    if internal_path.is_empty() {
        return None;
    }

    let path = std::path::Path::new(internal_path);
    if !path.exists() {
        return None;
    }

    // 1. 后缀名白名单过滤
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let is_doc = crate::vcp_modules::infra::file_extractor::is_extractable_extension(&ext);

    if !is_doc {
        return None;
    }

    log::debug!(
        "[FileManager] Self-Healing: Triggering real-time text extraction for hash={}",
        hash
    );

    // 2. 调起提取器进行自愈提取 (使用 spawn_blocking 隔离 CPU 密集型操作以防阻塞 Tokio 异步线程)
    let path_c = path.to_path_buf();
    let mime_c = mime_type.to_string();
    let text_opt = tokio::task::spawn_blocking(move || {
        crate::vcp_modules::infra::file_extractor::try_extract_text(&path_c, &mime_c)
    })
    .await
    .ok()
    .flatten();

    if let Some(text) = text_opt {
        let pool_c = pool.clone();
        let hash_c = hash.to_string();
        let text_c = text.clone();

        // 3. 异步持久化写入 SQLite，不阻塞当前的上下文加载请求
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE attachments SET extracted_text = ?, updated_at = ? WHERE hash = ?",
            )
            .bind(&text_c)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(&hash_c)
            .execute(&pool_c)
            .await;
        });

        Some(text)
    } else {
        None
    }
}

/// 强力物理删除指定的附件文件及其可能关联的原生硬解缩略图
pub async fn delete_attachment_physical<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    hash: &str,
    internal_path: &str,
) -> std::io::Result<()> {
    let path = std::path::Path::new(internal_path);
    if path.exists() {
        tokio::fs::remove_file(path).await?;
    }

    // 统一处理缩略图定位与删除
    let thumb_path = match get_thumbnails_root_dir(app_handle) {
        Ok(p) => p.join(format!("{}_thumb.webp", hash)),
        Err(_) => path
            .parent()
            .unwrap_or(path)
            .join("thumbnails")
            .join(format!("{}_thumb.webp", hash)),
    };
    if thumb_path.exists() {
        let _ = tokio::fs::remove_file(thumb_path).await;
    }

    // 统一处理多模态持久化缓存删除
    if let Ok(cache_dir) = get_multimodal_cache_dir(app_handle) {
        let cache_path = cache_dir.join(format!("{}.json", hash));
        if cache_path.exists() {
            let _ = tokio::fs::remove_file(cache_path).await;
        }
    }
    Ok(())
}

/// 统一校验附件格式支持情况 (合并多模态媒体白名单与文本提取文档白名单)
#[tauri::command]
pub fn check_attachment_support(original_name: String) -> Result<bool, String> {
    let ext = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if crate::vcp_modules::infra::file_extractor::is_supported_attachment_extension(&ext) {
        Ok(true)
    } else {
        Err(format!(
            "系统不支持 .{} 格式附件。\n大媒体（图片/视频/音频）支持直读多模态；文档（pdf/docx/xlsx/pptx）及常见代码和文本支持内容提取注入上下文。",
            ext
        ))
    }
}
