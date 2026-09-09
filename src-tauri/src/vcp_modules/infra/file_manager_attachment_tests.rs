use super::{
    canonical_file_within_root, check_existing_cas_size, check_existing_cas_size_async,
    normalize_attachment_mime, safe_storage_extension, validate_attachment_cas_path,
    verify_expected_hash, verify_file_sha256,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "file_manager_attachment_registration_tests.rs"]
mod registration_tests;

static XDG_CONFIG_HOME_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

pub(crate) async fn lock_xdg_config_home() -> tokio::sync::MutexGuard<'static, ()> {
    XDG_CONFIG_HOME_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn test_root(label: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("vcp-file-manager-{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("create test root");
    root
}

fn remove_test_root(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

#[test]
fn storage_extension_rejects_path_payloads() {
    assert_eq!(safe_storage_extension("报告.PDF"), Some("PDF"));
    assert_eq!(safe_storage_extension("archive.tar.gz"), Some("gz"));
    assert_eq!(safe_storage_extension("payload.a/b"), None);
    assert_eq!(safe_storage_extension("payload.very_long_extension"), None);
    assert_eq!(safe_storage_extension("payload.bad-ext"), None);
}

#[test]
fn canonical_staging_gate_rejects_parent_and_symlink_escape() {
    let root = test_root("staging");
    let uploads = root.join("uploads");
    let outside = root.join("outside");
    fs::create_dir_all(&uploads).expect("uploads");
    fs::create_dir_all(&outside).expect("outside");
    let staged = uploads.join("ok.bin");
    let escaped = outside.join("secret.bin");
    fs::write(&staged, b"ok").expect("staged");
    fs::write(&escaped, b"secret").expect("escaped");

    assert!(canonical_file_within_root(&uploads, &staged, "test").is_ok());
    assert!(canonical_file_within_root(&uploads, &escaped, "test").is_err());
    assert!(
        canonical_file_within_root(&uploads, &uploads.join("../outside/secret.bin"), "test")
            .is_err()
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&escaped, uploads.join("link.bin")).expect("symlink");
        assert!(canonical_file_within_root(&uploads, &uploads.join("link.bin"), "test").is_err());
    }
    remove_test_root(&root);
}

#[test]
fn cas_path_requires_hash_named_regular_file_and_exact_size() {
    let root = test_root("cas");
    let attachments = root.join("attachments");
    let outside = root.join("outside");
    fs::create_dir_all(&attachments).expect("attachments");
    fs::create_dir_all(&outside).expect("outside");
    let hash = "a".repeat(64);
    let valid = attachments.join(format!("{hash}.png"));
    fs::write(&valid, b"image").expect("valid CAS file");

    assert!(validate_attachment_cas_path(&attachments, &valid, &hash, 5).is_ok());
    assert!(validate_attachment_cas_path(&attachments, &valid, &"b".repeat(64), 5).is_err());
    assert!(validate_attachment_cas_path(&attachments, &valid, &hash, 4).is_err());
    assert!(validate_attachment_cas_path(
        &attachments,
        &attachments.join("../outside/file.png"),
        &hash,
        5,
    )
    .is_err());

    #[cfg(unix)]
    {
        let link = attachments.join(format!("{}.png", "c".repeat(64)));
        std::os::unix::fs::symlink(&valid, &link).expect("CAS symlink");
        assert!(validate_attachment_cas_path(&attachments, &link, &"c".repeat(64), 5).is_err());
    }
    remove_test_root(&root);
}

#[test]
fn native_hash_is_checked_against_rust_recomputation() {
    let actual = "a".repeat(64);
    assert!(verify_expected_hash(Some(&actual), &actual).is_ok());
    assert!(verify_expected_hash(Some(&"b".repeat(64)), &actual).is_err());
    assert!(verify_expected_hash(None, &actual).is_ok());
}

#[test]
fn attachment_mime_is_normalized_without_path_or_control_syntax() {
    assert_eq!(
        normalize_attachment_mime("Image/PNG; charset=binary").expect("valid MIME"),
        "image/png"
    );
    for invalid in ["image", "image/", "/png", "image/png\nsecret", "image/p ng"] {
        assert!(normalize_attachment_mime(invalid).is_err(), "{invalid}");
    }
}

#[tokio::test]
async fn existing_cas_reuse_requires_regular_file_and_exact_size() {
    let root = test_root("existing");
    let path = root.join("cas.bin");
    fs::write(&path, b"complete").expect("complete CAS");
    assert!(check_existing_cas_size(&path, 8).is_ok());
    assert!(check_existing_cas_size_async(&path, 8).await.is_ok());
    fs::write(&path, b"short").expect("truncated CAS");
    assert!(check_existing_cas_size(&path, 8).is_err());
    assert!(check_existing_cas_size_async(&path, 8).await.is_err());

    #[cfg(unix)]
    {
        let target = root.join("target.bin");
        let link = root.join("link.bin");
        fs::write(&target, b"complete").expect("target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(check_existing_cas_size(&link, 8).is_err());
        assert!(check_existing_cas_size_async(&link, 8).await.is_err());
    }
    remove_test_root(&root);
}

#[tokio::test]
async fn cas_content_is_rehashed_before_binding() {
    let root = test_root("content-hash");
    let path = root.join("attachment.bin");
    fs::write(&path, b"trusted bytes").expect("CAS bytes");
    let expected = crate::vcp_modules::infra::utils::calculate_sha256(b"trusted bytes");

    assert!(verify_file_sha256(&path, &expected).await.is_ok());
    assert!(verify_file_sha256(&path, &expected.to_ascii_uppercase())
        .await
        .is_ok());
    fs::write(&path, b"tampered byte").expect("tampered CAS bytes");
    assert!(verify_file_sha256(&path, &expected).await.is_err());

    remove_test_root(&root);
}
