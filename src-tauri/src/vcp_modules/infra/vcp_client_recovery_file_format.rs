use std::path::{Path, PathBuf};

pub(crate) fn find_claimed_files(cache: &Path, token: &str) -> Result<Vec<PathBuf>, String> {
    let prefix = format!("sse_recovered_{token}.claimed.");
    let entries = match std::fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取恢复缓存目录失败: {error}")),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取恢复缓存条目失败: {error}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && path_is_file(&path)? {
            candidates.push(path);
        }
    }
    candidates.sort();
    Ok(candidates)
}

pub(crate) fn parse_claim_generation(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let marker = name.split_once(".claimed.g")?.1;
    let parts: Vec<_> = marker.split('.').collect();
    if parts.len() != 4
        || !parts[1].starts_with('e')
        || !parts[2].starts_with('p')
        || !parts[3].starts_with('s')
    {
        return None;
    }
    let generation = parts[0].parse::<u64>().ok().filter(|value| *value > 0)?;
    for component in &parts[1..] {
        component[1..]
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)?;
    }
    Some(generation)
}

pub(crate) fn read_recovery_generation(path: &Path) -> Result<Option<u64>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("读取恢复文件 generation 失败: {error}"))?;
    let value = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                "[VCPClient] 恢复文件 JSON 损坏，转入 discarded：error={error}; result=discarded"
            );
            return Ok(None);
        }
    };
    Ok(value
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .filter(|generation| *generation > 0))
}

pub(crate) fn remove_claimed_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理旧 claimed 文件失败: {error}")),
    }
}

pub(crate) fn path_exists(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("检查恢复文件失败: {error}")),
    }
}

pub(crate) fn path_is_file(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("检查恢复文件失败: {error}")),
    }
}

pub(crate) fn stable_stream_identity_token(
    key: &crate::vcp_modules::chat::topic_types::MessageKey,
) -> String {
    let canonical = [
        key.topic.owner_type.as_str(),
        key.topic.owner_id.as_str(),
        key.topic.topic_id.as_str(),
        key.msg_id.as_str(),
    ]
    .into_iter()
    .map(|value| {
        let length = value.len();
        format!("{length}:{value}")
    })
    .collect::<String>();
    crate::vcp_modules::infra::utils::calculate_sha256(canonical.as_bytes())
}

pub(crate) fn read_recovery_payload(
    path: &Path,
) -> Result<(String, i64, Option<String>, u64), String> {
    let content =
        std::fs::read_to_string(path).map_err(|error| format!("读取恢复文件失败: {error}"))?;
    let value = serde_json::from_str::<serde_json::Value>(&content)
        .map_err(|error| format!("解析恢复文件失败: {error}"))?;
    let recovered_content = value
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "恢复文件缺少 content".to_string())?;
    let timestamp = value
        .get("timestamp")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "恢复文件缺少 timestamp".to_string())?;
    let generation = value
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "恢复文件缺少有效 generation".to_string())?;
    Ok((
        recovered_content.to_string(),
        timestamp,
        value["finishReason"].as_str().map(str::to_string),
        generation,
    ))
}
