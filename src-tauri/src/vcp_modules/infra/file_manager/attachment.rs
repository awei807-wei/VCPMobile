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

pub use ingest::{get_attachment_real_path, register_local_file, store_file};
pub use paths::{
    get_attachments_root_dir, get_data_root_dir, get_multimodal_cache_dir, get_thumbnails_root_dir,
    safe_rename,
};
pub(crate) use registration::commit_registered_attachment;
pub use registration::{register_attachment_internal, AttachmentData};
pub(crate) use validation::{
    canonical_file_within_root, check_existing_cas_size, check_existing_cas_size_async,
    normalize_attachment_mime, resolve_attachment_cas_file, safe_storage_extension,
    store_file_semaphore, validate_attachment_cas_path, verify_expected_hash, verify_file_sha256,
};
