use std::path::Path;

#[cfg(unix)]
pub(super) fn remove_indexed_path_sync(root: &Path, raw_path: &str) -> Result<bool, String> {
    let (_, state) = super::validate_indexed_path(root, raw_path)?;
    if state == super::ManagedPathState::Missing {
        return Ok(false);
    }
    let clean = raw_path.strip_prefix("file://").unwrap_or(raw_path);
    secure_unlink_regular_file(root, Path::new(clean))
        .map_err(|error| format!("附件索引文件安全删除失败: {error}"))
}

#[cfg(not(unix))]
pub(super) fn remove_indexed_path_sync(_root: &Path, _raw_path: &str) -> Result<bool, String> {
    Err("当前平台不支持附件路径的安全相对删除".to_string())
}

#[cfg(unix)]
pub(super) fn remove_candidate(root: &Path, candidate: &Path) -> Result<bool, String> {
    secure_unlink_regular_file(root, candidate)
        .map_err(|error| format!("附件幽灵文件安全删除失败: {error}"))
}

#[cfg(not(unix))]
pub(super) fn remove_candidate(_root: &Path, _candidate: &Path) -> Result<bool, String> {
    Err("当前平台不支持附件幽灵文件的安全相对删除".to_string())
}

#[cfg(unix)]
fn secure_unlink_regular_file(root: &Path, candidate: &Path) -> std::io::Result<bool> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let canonical_root = super::canonical_managed_root(root).map_err(std::io::Error::other)?;
    if !candidate.is_absolute() {
        return Err(std::io::Error::other("附件删除路径必须是绝对路径"));
    }
    let relative = candidate
        .strip_prefix(&canonical_root)
        .map_err(|_| std::io::Error::other("附件删除路径不在受管 root 内"))?;
    let mut components = relative.components();
    let file_name = match components.next_back() {
        Some(std::path::Component::Normal(name)) => name,
        _ => return Err(std::io::Error::other("附件删除路径缺少 regular 文件名")),
    };
    let root_fd = open_dir_fd(&canonical_root)?;
    let mut parent_fd = root_fd;
    for component in components {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::other("附件删除路径包含非法组件"));
        };
        parent_fd = open_child_dir_fd(parent_fd.as_raw_fd(), name)?;
    }
    let name = CString::new(file_name.as_bytes())
        .map_err(|_| std::io::Error::other("附件文件名包含 NUL"))?;
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let stat_result = unsafe {
        libc::fstatat(
            parent_fd.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if stat_result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error)
        };
    }
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(std::io::Error::other("附件删除目标不是 regular file"));
    }
    let unlink_result = unsafe { libc::unlinkat(parent_fd.as_raw_fd(), name.as_ptr(), 0) };
    if unlink_result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(true)
}

#[cfg(unix)]
fn open_dir_fd(path: &Path) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let value = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("附件受管 root 包含 NUL"))?;
    let fd = unsafe {
        libc::open(
            value.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_child_dir_fd(
    parent_fd: std::os::fd::RawFd,
    name: &std::ffi::OsStr,
) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let value =
        CString::new(name.as_bytes()).map_err(|_| std::io::Error::other("附件路径组件包含 NUL"))?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            value.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}
