use tauri::Manager;

/// 核心路径解析：获取基础数据存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files
pub fn get_data_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    // document_dir 在 Android 上通常指向 .../files/documents
    let mut path = app_handle
        .path()
        .document_dir()
        .map_err(|e| format!("Failed to get document_dir: {}", e))?;
    path.pop(); // 弹出 documents，留下 .../files
    Ok(path)
}

/// 核心路径解析：获取附件存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/attachments
pub fn get_attachments_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("attachments");
    Ok(path)
}

/// 核心路径解析：获取缩略图存储根目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/thumbnails
pub fn get_thumbnails_root_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("thumbnails");
    Ok(path)
}

/// 核心路径解析：获取多模态抽取/转码持久化缓存目录
/// Android: /storage/emulated/0/Android/data/<pkg>/files/multimodal_cache
pub fn get_multimodal_cache_dir<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    let mut path = get_data_root_dir(app_handle)?;
    path.push("multimodal_cache");
    Ok(path)
}

/// 物理安全的文件重命名工具，能够跨越物理挂载分区 (EXDEV) 降级进行物理拷贝+删除
pub fn safe_rename<P: AsRef<std::path::Path>, Q: AsRef<std::path::Path>>(
    from: P,
    to: Q,
) -> std::io::Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();

    if std::fs::rename(from, to).is_err() {
        // 跨分区时先复制到目标目录的唯一临时文件，再原子提交，禁止正式路径出现半文件。
        let parent = to.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "附件目标缺少父目录")
        })?;
        let temporary = parent.join(format!(".ingest-{}.tmp", uuid::Uuid::new_v4()));
        if let Err(error) = std::fs::copy(from, &temporary) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = std::fs::rename(&temporary, to) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        let _ = std::fs::remove_file(from);
    }
    Ok(())
}
