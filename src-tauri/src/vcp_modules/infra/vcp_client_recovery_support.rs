/// Remove stale recovery files without touching files that may still be
/// claimed by an active generation.
pub(super) fn clean_old_cache_files(cache_dir: &std::path::Path) {
    let sse_cache_dir = cache_dir.join("sse_cache");
    if !sse_cache_dir.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(sse_cache_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(elapsed) = modified.elapsed() else {
            continue;
        };
        if elapsed.as_secs() > 24 * 3600 {
            log::info!("[VCPClient] 删除超过 24 小时的孤立恢复缓存：result=deleted");
            let _ = std::fs::remove_file(path);
        }
    }
}
