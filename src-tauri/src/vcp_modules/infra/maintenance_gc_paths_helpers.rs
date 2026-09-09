use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::vcp_modules::infra::utils::is_valid_cas_hash;

use super::{
    canonical_managed_root, resolve_reference_target_through_symlinks, validate_indexed_path,
    validate_reference_path, ManagedPathState, RootKind,
};

pub(crate) fn is_managed_attachment_temp(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".tmp") else {
        return false;
    };
    if let Some(value) = stem.strip_prefix(".ingest-") {
        return uuid::Uuid::parse_str(value).is_ok() || is_hash_uuid_temp(value);
    }
    stem.strip_prefix(".thumb-").is_some_and(is_hash_uuid_temp)
}

fn is_hash_uuid_temp(value: &str) -> bool {
    let Some(hash) = value.get(..64) else {
        return false;
    };
    is_valid_cas_hash(hash)
        && value.as_bytes().get(64) == Some(&b'-')
        && value
            .get(65..)
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}

pub(crate) fn scan_cursor_key(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Attachment => super::super::db::ATTACHMENT_SCAN_CURSOR_KEY,
        RootKind::Thumbnail => super::super::db::THUMBNAIL_SCAN_CURSOR_KEY,
        RootKind::MultimodalCache => super::super::db::MULTIMODAL_CACHE_SCAN_CURSOR_KEY,
    }
}

pub(crate) fn root_kind_key(kind: RootKind) -> &'static str {
    match kind {
        RootKind::Attachment => "attachment",
        RootKind::Thumbnail => "thumbnail",
        RootKind::MultimodalCache => "multimodal_cache",
    }
}

pub(crate) fn root_kind_from_key(value: &str) -> Option<RootKind> {
    match value {
        "attachment" => Some(RootKind::Attachment),
        "thumbnail" => Some(RootKind::Thumbnail),
        "multimodal_cache" => Some(RootKind::MultimodalCache),
        _ => None,
    }
}

/// Normalize a persisted lexical cursor. Invalid values are treated as an
/// exhausted/old cursor and reset to the root rather than being interpreted
/// as a path outside the managed directory.
pub(crate) fn normalize_scan_cursor(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let path = Path::new(raw);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return String::new();
    }
    raw.to_string()
}

/// Normalize a persisted unlink relative path without altering filename
/// bytes. Unlike the scan cursor, an unlink path is a filesystem name and
/// leading/trailing spaces are significant. Reject empty, absolute, and any
/// non-normal component so callers cannot turn a debt into a traversal.
pub(crate) fn normalize_unlink_relative_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("附件 unlink 相对路径不能为空".to_string());
    }
    let path = Path::new(raw);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("附件 unlink 相对路径包含非法组件".to_string());
    }
    Ok(raw.to_string())
}

pub(crate) fn relative_path_for_root(root: &Path, path: &Path) -> Result<String, String> {
    let canonical_root = canonical_managed_root(root)?;
    let canonical_path = std::fs::canonicalize(path)
        .map_err(|error| format!("附件 unlink 路径规范化失败: {error}"))?;
    relative_path_for_validated_root(&canonical_root, &canonical_path)
}

/// Convert a path already validated by the reference resolver to a managed
/// relative path. Unlike relative_path_for_root, this also accepts a
/// missing final file, which is useful when clearing a stale unlink debt for
/// a live database reference.
pub(crate) fn relative_path_for_validated_root(root: &Path, path: &Path) -> Result<String, String> {
    let canonical_root = canonical_managed_root(root)?;
    let relative = path
        .strip_prefix(&canonical_root)
        .map_err(|_| "附件 unlink 路径不在受管 root 内".to_string())?;
    let relative = relative_to_string(relative)?;
    if relative.is_empty() {
        return Err("附件 unlink 相对路径不能为空".to_string());
    }
    Ok(relative)
}

/// 将一个已存在的附件引用解析为真实 managed root 下唯一的 outbox 相对键。
///
/// 生产写入口必须传入由应用路径配置得到的真实 root。这里复用严格的
/// `validate_reference_path`：root 及其祖先不得是 symlink，引用的父目录也不得
/// 穿过 symlink；合法的 `legacy/../nested` 词法别名仍会规范化到同一个物理文件。
/// 无法证明路径属于该 root，或目标不是当前存在的 regular file 时，返回 None，
/// 调用方保留债务而不冒险清除。
pub(crate) fn live_reference_unlink_relative_path(root: &Path, raw_path: &str) -> Option<String> {
    let (canonical_target, state) = validate_reference_path(root, raw_path).ok()?;
    if state != ManagedPathState::Present {
        return None;
    }
    let canonical_root = canonical_managed_root(root).ok()?;
    relative_path_for_validated_root(&canonical_root, &canonical_target).ok()
}

/// Resolve a live path through symlinks to the unique relative key of the
/// declared managed root. This is intentionally separate from the strict
/// write-side helper: callers use it only to persist a bounded witness for a
/// debt that must remain until the outbox transaction rechecks the live hash.
pub(crate) fn live_reference_unlink_relative_path_through_symlinks(
    root: &Path,
    raw_path: &str,
) -> Option<String> {
    let (canonical_target, state) =
        resolve_reference_target_through_symlinks(root, raw_path).ok()?;
    if state != ManagedPathState::Present {
        return None;
    }
    let canonical_root = canonical_managed_root(root).ok()?;
    relative_path_for_validated_root(&canonical_root, &canonical_target).ok()
}

pub(crate) fn resolve_relative_indexed_path(
    root: &Path,
    relative: &str,
) -> Result<(PathBuf, ManagedPathState), String> {
    let canonical_root = canonical_managed_root(root)?;
    let relative = normalize_unlink_relative_path(relative)?;
    let candidate = canonical_root.join(relative);
    let candidate = candidate
        .to_str()
        .ok_or_else(|| "附件 unlink 路径不是有效 UTF-8".to_string())?;
    validate_indexed_path(&canonical_root, candidate)
}

fn validate_relative_path(relative: &str) -> Result<(), String> {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("附件 unlink 相对路径包含非法组件".to_string());
    }
    Ok(())
}

pub(crate) fn relative_to_string(path: &Path) -> Result<String, String> {
    validate_relative_path(
        path.to_str()
            .ok_or_else(|| "附件相对路径不是有效 UTF-8".to_string())?,
    )?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "附件相对路径不是有效 UTF-8".to_string())
}

pub(crate) fn should_expire_temp(
    modified: Result<SystemTime, std::io::Error>,
    now: SystemTime,
    grace: Duration,
) -> bool {
    let Ok(modified) = modified else {
        return false;
    };
    now.duration_since(modified)
        .map(|age| age >= grace)
        .unwrap_or(false)
}

pub(crate) fn managed_hash_from_file_name(name: &str, kind: RootKind) -> Option<&str> {
    match kind {
        RootKind::Attachment => Path::new(name)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|hash| is_valid_cas_hash(hash)),
        RootKind::Thumbnail => name
            .strip_suffix("_thumb.webp")
            .filter(|hash| is_valid_cas_hash(hash)),
        RootKind::MultimodalCache => name
            .strip_suffix(".json")
            .filter(|hash| is_valid_cas_hash(hash)),
    }
}
