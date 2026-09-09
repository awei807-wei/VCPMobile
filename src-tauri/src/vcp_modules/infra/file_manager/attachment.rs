#[path = "attachment/ingest.rs"]
mod ingest;
#[path = "attachment/ingest_helpers.rs"]
mod ingest_helpers;
#[path = "attachment/paths.rs"]
mod paths;
#[path = "attachment/registration.rs"]
mod registration;
#[path = "attachment/validation.rs"]
mod validation;

use std::sync::OnceLock;
use tokio::sync::{RwLock, RwLockReadGuard};

pub(crate) type AttachmentReadGuard = RwLockReadGuard<'static, ()>;

pub use ingest::{get_attachment_real_path, register_local_file, store_file};
pub use paths::{
    get_attachments_root_dir, get_data_root_dir, get_multimodal_cache_dir, get_thumbnails_root_dir,
    safe_rename,
};
pub(crate) use registration::{
    commit_registered_attachment, commit_registered_attachment_unlocked,
    register_attachment_internal_unlocked, AttachmentRegistrationInput,
};
pub use registration::{register_attachment_internal, AttachmentData};
pub(crate) use validation::{
    canonical_file_within_root, check_existing_cas_size, check_existing_cas_size_async,
    normalize_attachment_mime, resolve_attachment_cas_file, safe_storage_extension,
    store_file_semaphore, validate_attachment_cas_path, verify_expected_hash, verify_file_sha256,
};

/// 附件注册与冷启动 GC 的进程内闸门。
///
/// 注册路径持有读锁，GC 持有写锁并覆盖“数据库 CAS 检查到物理删除”的整个窗口，
/// 避免本进程新引用在 GC 删除正式文件之后才提交。数据库事务仍负责跨连接的最终
/// 引用检查；两层协议共同保证普通上传和同步写入不会复活到缺失文件。
pub(crate) fn attachment_gc_gate() -> &'static RwLock<()> {
    static GATE: OnceLock<RwLock<()>> = OnceLock::new();
    GATE.get_or_init(|| RwLock::new(()))
}

/// 为 rusqlite 的同步写队列提供阻塞式读锁，和异步注册入口共用同一闸门。
pub(crate) fn attachment_gc_gate_blocking_read() -> tokio::sync::RwLockReadGuard<'static, ()> {
    attachment_gc_gate().blocking_read()
}
