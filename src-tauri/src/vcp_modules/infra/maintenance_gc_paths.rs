use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[path = "maintenance_gc_paths_helpers.rs"]
mod helpers;
#[path = "maintenance_gc_scan.rs"]
mod scan;
#[path = "maintenance_gc_secure.rs"]
mod secure;
#[path = "maintenance_gc_sweep.rs"]
mod sweep;
#[cfg(test)]
pub(super) use helpers::normalize_unlink_relative_path;
pub(super) use helpers::{
    is_managed_attachment_temp, live_reference_unlink_relative_path,
    live_reference_unlink_relative_path_through_symlinks, managed_hash_from_file_name,
    normalize_scan_cursor, relative_path_for_root, relative_path_for_validated_root,
    relative_to_string, resolve_relative_indexed_path, root_kind_from_key, root_kind_key,
    scan_cursor_key, should_expire_temp,
};
pub(super) use sweep::sweep_root_after_commit;
#[cfg(test)]
pub(super) use sweep::sweep_root_after_commit_for_test;
#[cfg(test)]
pub(super) use sweep::sweep_snapshot_page_for_test;

const MAX_SCANNED_FILES_PER_ROOT: usize = 4096;
const ATTACHMENT_TEMP_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// 附件 GC 的三个受管目录。目录之外的数据库路径永远不允许被 GC 触碰。
#[derive(Debug, Clone)]
pub(crate) struct ManagedAttachmentRoots {
    pub(crate) attachments: PathBuf,
    pub(crate) thumbnails: PathBuf,
    pub(crate) multimodal_cache: PathBuf,
}

impl ManagedAttachmentRoots {
    pub(crate) fn new(
        attachments: PathBuf,
        thumbnails: PathBuf,
        multimodal_cache: PathBuf,
    ) -> Self {
        Self {
            attachments,
            thumbnails,
            multimodal_cache,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedPathState {
    Missing,
    Present,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RootKind {
    Attachment,
    Thumbnail,
    MultimodalCache,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SweepReport {
    pub(super) removed: usize,
    pub(super) inspected: usize,
    pub(super) has_more: bool,
}

pub(super) fn canonical_managed_root(root: &Path) -> Result<PathBuf, String> {
    // Do not use Path::canonicalize here. It follows every symlink in the
    // root, including an ancestor, and would turn an escaped directory into a
    // seemingly valid managed root. Walk the lexical absolute path one
    // component at a time with symlink_metadata so every existing component is
    // explicitly trusted. Once a normal component is missing, preserve the
    // remaining lexical suffix as a synthetic empty root for cold-start scans.
    let absolute_root = absolute_path(root)?;
    let mut current = PathBuf::new();
    for component in absolute_root.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !current.pop() {
                    return Err("附件受管根目录路径逃逸文件系统根目录".to_string());
                }
            }
            Component::Normal(name) => {
                current.push(name);
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err("附件受管根目录或其祖先不得是 symlink".to_string());
                    }
                    Ok(metadata) if !metadata.file_type().is_dir() => {
                        return Err("附件受管根目录不是 regular directory".to_string());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("附件受管根目录不可用: {error}")),
                }
            }
        }
    }
    if current.is_absolute() {
        Ok(current)
    } else {
        Err("附件受管根目录必须是绝对路径".to_string())
    }
}

fn lexical_absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = absolute_path(path)?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("附件路径逃逸文件系统根目录".to_string());
                }
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err("附件路径必须是绝对路径".to_string())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("获取附件受管根目录基路径失败: {error}"))?
            .join(path)
    };
    if absolute.is_absolute() {
        Ok(absolute)
    } else {
        Err("附件路径必须是绝对路径".to_string())
    }
}

fn validate_managed_parent(root: &Path, parent: &Path) -> Result<PathBuf, String> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| "附件索引路径不在受管 root 内".to_string())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(name) => {
                current.push(name);
                let metadata = std::fs::symlink_metadata(&current)
                    .map_err(|error| format!("附件索引父目录不可用: {error}"))?;
                if !metadata.file_type().is_dir() {
                    return Err("附件索引路径父级不是 regular directory".to_string());
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if current == root || !current.pop() {
                    return Err("附件索引路径逃逸受管 root".to_string());
                }
            }
            _ => return Err("附件索引路径包含非法的相对组件".to_string()),
        }
    }
    let canonical =
        std::fs::canonicalize(parent).map_err(|error| format!("附件索引父目录不可用: {error}"))?;
    if !canonical.starts_with(root) {
        return Err("附件索引路径不在受管 root 内".to_string());
    }
    Ok(canonical)
}

pub(super) fn path_may_be_inside_managed_root(root: &Path, raw_path: &str) -> bool {
    let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
    let candidate = Path::new(clean_path);
    candidate.is_absolute()
        && lexical_absolute_path(candidate)
            .ok()
            .is_some_and(|candidate| candidate.starts_with(root))
}

/// Resolve an indexed reference to its physical path. Unlike
/// `validate_indexed_path`, this helper permits a final symlink when it points
/// to a regular file inside the same managed root. It is used only for
/// reference accounting; deletion still uses the stricter validator and the
/// descriptor-relative secure unlink path.
pub(super) fn validate_reference_path(
    root: &Path,
    raw_path: &str,
) -> Result<(PathBuf, ManagedPathState), String> {
    let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
    let candidate = Path::new(clean_path);
    if !candidate.is_absolute() {
        return Err("附件索引路径必须是绝对路径".to_string());
    }
    validate_reference_path_from_root(root, candidate)
}

/// Resolve a reference by following all existing symlink components. This is
/// intentionally separate from validate_reference_path: the latter rejects
/// unsafe lexical parents for ordinary reference accounting, while a
/// committed unlink debt may still need to be cleared when a live record uses
/// a root-external alias or a symlinked ancestor that resolves to the same
/// managed regular file.
pub(super) fn resolve_reference_target_through_symlinks(
    root: &Path,
    raw_path: &str,
) -> Result<(PathBuf, ManagedPathState), String> {
    let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
    let candidate = Path::new(clean_path);
    if !candidate.is_absolute() {
        return Err("附件索引路径必须是绝对路径".to_string());
    }
    let canonical_root = canonical_managed_root(root)?;
    let canonical = std::fs::canonicalize(candidate)
        .map_err(|error| format!("附件索引 symlink 目标规范化失败: {error}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("附件索引 symlink 目标不在受管 root 内".to_string());
    }
    match std::fs::symlink_metadata(&canonical) {
        Ok(metadata) if metadata.file_type().is_file() => {
            Ok((canonical, ManagedPathState::Present))
        }
        Ok(_) => Err("附件索引 symlink 目标不是 regular file".to_string()),
        Err(error) => Err(format!("附件索引 symlink 目标不可检查: {error}")),
    }
}

pub(super) fn validate_reference_path_from_root(
    canonical_root: &Path,
    candidate: &Path,
) -> Result<(PathBuf, ManagedPathState), String> {
    let parent = candidate
        .parent()
        .ok_or_else(|| "附件索引路径缺少父目录".to_string())?;
    let file_name = candidate
        .file_name()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "附件索引路径缺少文件名".to_string())?;
    let canonical_parent = validate_managed_parent(canonical_root, parent)?;
    let canonical_candidate = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            let canonical = std::fs::canonicalize(candidate)
                .map_err(|error| format!("附件索引文件规范化失败: {error}"))?;
            if !canonical.starts_with(canonical_root) {
                return Err("附件索引文件通过 symlink 逃逸受管 root".to_string());
            }
            let target_metadata = std::fs::symlink_metadata(&canonical)
                .map_err(|error| format!("附件索引 symlink 目标不可检查: {error}"))?;
            if !target_metadata.file_type().is_file() {
                return Err("附件索引路径不是 regular file".to_string());
            }
            canonical
        }
        Ok(_) => return Err("附件索引路径不是 regular file".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            canonical_parent.join(file_name)
        }
        Err(error) => return Err(format!("附件索引文件不可检查: {error}")),
    };
    let state = match std::fs::symlink_metadata(&canonical_candidate) {
        Ok(metadata) if metadata.file_type().is_file() => ManagedPathState::Present,
        Ok(_) => return Err("附件索引路径不是 regular file".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ManagedPathState::Missing,
        Err(error) => return Err(format!("附件索引文件不可检查: {error}")),
    };
    Ok((canonical_candidate, state))
}

/// 只接受受管 root 下的 regular file；不会跟随最终文件或父目录中的 symlink。
///
/// 返回值中的路径已经 canonicalize，可直接用于后续删除。目标不存在时仍会校验其
/// canonical 父目录，以便把“已被外部删除”的索引视为幂等成功，而不是放宽路径边界。
pub(super) fn validate_indexed_path(
    root: &Path,
    raw_path: &str,
) -> Result<(PathBuf, ManagedPathState), String> {
    let clean_path = raw_path.strip_prefix("file://").unwrap_or(raw_path);
    let candidate = Path::new(clean_path);
    if !candidate.is_absolute() {
        return Err("附件索引路径必须是绝对路径".to_string());
    }
    let canonical_root = canonical_managed_root(root)?;
    let parent = candidate
        .parent()
        .ok_or_else(|| "附件索引路径缺少父目录".to_string())?;
    let file_name = candidate
        .file_name()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "附件索引路径缺少文件名".to_string())?;
    let canonical_parent = validate_managed_parent(&canonical_root, parent)?;

    let canonical_candidate = match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                return Err("附件索引路径不是 regular file".to_string());
            }
            let canonical = std::fs::canonicalize(candidate)
                .map_err(|error| format!("附件索引文件规范化失败: {error}"))?;
            if !canonical.starts_with(&canonical_root) {
                return Err("附件索引文件通过 symlink 逃逸受管 root".to_string());
            }
            canonical
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            canonical_parent.join(file_name)
        }
        Err(error) => return Err(format!("附件索引文件不可检查: {error}")),
    };
    let state = match std::fs::symlink_metadata(&canonical_candidate) {
        Ok(metadata) if metadata.file_type().is_file() => ManagedPathState::Present,
        Ok(_) => return Err("附件索引路径不是 regular file".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ManagedPathState::Missing,
        Err(error) => return Err(format!("附件索引文件不可检查: {error}")),
    };
    Ok((canonical_candidate, state))
}

/// 再次确认并删除一个已校验的索引路径。Unix 使用目录 fd 相对删除，拒绝父级 symlink。
pub(super) async fn remove_indexed_path(root: &Path, raw_path: &str) -> Result<bool, String> {
    let root = root.to_path_buf();
    let raw_path = raw_path.to_string();
    tokio::task::spawn_blocking(move || secure::remove_indexed_path_sync(&root, &raw_path))
        .await
        .map_err(|error| format!("附件索引文件删除任务失败: {error}"))?
}

#[cfg(test)]
#[path = "maintenance_gc_paths_tests.rs"]
mod tests;
