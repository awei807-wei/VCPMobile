#[path = "file_manager/attachment.rs"]
mod attachment;
#[path = "file_manager/legacy.rs"]
mod legacy;

#[allow(unused_imports)]
pub use attachment::{
    get_attachment_real_path, get_attachments_root_dir, get_data_root_dir,
    get_multimodal_cache_dir, get_thumbnails_root_dir, register_attachment_internal,
    register_local_file, safe_rename, store_file, AttachmentData,
};
#[allow(unused_imports)]
pub use legacy::{
    check_attachment_support, clear_upload_cache, delete_attachment_physical,
    ensure_extracted_text, evict_multimodal_cache_if_needed, generate_thumbnail,
    get_refined_mime_type, open_file, resolve_attachment_path,
};

#[allow(unused_imports)]
pub(crate) use attachment::{
    attachment_gc_gate, attachment_gc_gate_blocking_read, canonical_file_within_root,
    check_existing_cas_size, check_existing_cas_size_async, commit_registered_attachment,
    commit_registered_attachment_unlocked, normalize_attachment_mime,
    register_attachment_internal_unlocked, resolve_attachment_cas_file, safe_storage_extension,
    store_file_semaphore, validate_attachment_cas_path, verify_expected_hash, verify_file_sha256,
    AttachmentReadGuard, AttachmentRegistrationInput,
};

#[cfg(test)]
#[path = "file_manager_attachment_tests.rs"]
mod attachment_tests;

#[cfg(test)]
pub(crate) async fn lock_attachment_test_environment() -> tokio::sync::MutexGuard<'static, ()> {
    attachment_tests::lock_xdg_config_home().await
}
