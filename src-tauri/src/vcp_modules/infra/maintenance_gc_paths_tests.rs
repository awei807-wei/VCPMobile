use super::scan::scan_managed_root_from_cursor;
use super::sweep::{sweep_managed_root, ManagedRootSweepOptions};
use super::{
    is_managed_attachment_temp, live_reference_unlink_relative_path,
    normalize_unlink_relative_path, should_expire_temp, validate_indexed_path, ManagedPathState,
    RootKind,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn test_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "vcp-attachment-gc-paths-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("创建测试目录");
    root
}

fn scan_relatives(root: &Path, page: &super::scan::ScanPage) -> Vec<String> {
    let canonical_root = fs::canonicalize(root).expect("规范化扫描根目录");
    page.candidates
        .iter()
        .map(|candidate| {
            super::relative_to_string(
                candidate
                    .path
                    .strip_prefix(&canonical_root)
                    .expect("扫描候选应位于根目录内"),
            )
            .expect("扫描候选相对路径应为 UTF-8")
        })
        .collect()
}

#[test]
fn managed_temp_names_are_strict() {
    assert!(is_managed_attachment_temp(
        ".ingest-123e4567-e89b-12d3-a456-426614174000.tmp"
    ));
    assert!(is_managed_attachment_temp(&format!(
        ".thumb-{}-123e4567-e89b-12d3-a456-426614174000.tmp",
        "a".repeat(64)
    )));
    assert!(!is_managed_attachment_temp(".ingest-attacker.tmp"));
    assert!(!is_managed_attachment_temp(".thumb-not-a-hash.tmp"));
    assert!(!is_managed_attachment_temp(
        ".ingest-123e4567-e89b-12d3-a456-426614174000"
    ));
}

#[test]
fn unlink_relative_paths_preserve_spaces_and_reject_traversal() {
    assert_eq!(
        normalize_unlink_relative_path("file.bin ").expect("合法尾部空格文件名"),
        "file.bin "
    );
    for invalid in [
        "",
        "/absolute/file.bin",
        "../file.bin",
        "nested/../file.bin",
        "./file.bin",
    ] {
        assert!(
            normalize_unlink_relative_path(invalid).is_err(),
            "应拒绝 {invalid:?}"
        );
    }
}

#[test]
fn live_reference_path_uses_only_the_declared_managed_root() {
    let root = test_root("nested-same-named-root");
    let managed_root = root.join("attachments");
    let nested_root = root.join("attachments/nested/attachments");
    let target = nested_root.join("shared.bin");
    fs::create_dir_all(&nested_root).expect("创建嵌套同名 root");
    fs::write(&target, b"nested shared").expect("写入嵌套同名 root 文件");

    assert_eq!(
        live_reference_unlink_relative_path(&managed_root, &target.to_string_lossy()),
        Some("nested/attachments/shared.bin".to_string()),
        "必须只返回真实外层 attachments root 的债务键"
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn live_reference_path_rejects_external_or_ancestor_symlink_aliases() {
    let root = test_root("symlink-alias");
    let outside = test_root("symlink-alias-outside");
    let managed_root = root.join("attachments");
    let target = managed_root.join("shared.bin");
    fs::create_dir_all(&managed_root).expect("创建受管 root");
    fs::write(&target, b"managed").expect("写入受管文件");
    let external_alias = outside.join("shared.bin");
    std::os::unix::fs::symlink(&target, &external_alias).expect("创建 root 外 alias");
    let ancestor = managed_root.join("alias-parent");
    std::os::unix::fs::symlink(&outside, &ancestor).expect("创建祖先 symlink");

    assert_eq!(
        live_reference_unlink_relative_path(&managed_root, &external_alias.to_string_lossy()),
        None,
        "root 外 alias 不得证明为受管相对键"
    );
    assert_eq!(
        live_reference_unlink_relative_path(
            &managed_root,
            &ancestor.join("shared.bin").to_string_lossy()
        ),
        None,
        "祖先 symlink 不得证明为受管相对键"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[tokio::test]
async fn scan_orders_directory_and_file_by_relative_string_cursor() {
    let root = test_root("scan-prefix-order");
    fs::create_dir(root.join("H")).expect("创建同前缀目录");
    fs::write(root.join("H/file.bin"), b"nested").expect("写入嵌套文件");
    fs::write(root.join("H.bin"), b"flat").expect("写入同前缀文件");

    let first = scan_managed_root_from_cursor(&root, "", 1).await;
    assert_eq!(scan_relatives(&root, &first), vec!["H.bin"]);
    assert!(first.has_more);
    assert_eq!(first.next_cursor, "H.bin");

    let second = scan_managed_root_from_cursor(&root, &first.next_cursor, 1).await;
    assert_eq!(scan_relatives(&root, &second), vec!["H/file.bin"]);
    assert!(!second.has_more);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn scan_uses_global_relative_order_despite_read_dir_order() {
    let root = test_root("scan-global-order");
    fs::create_dir(root.join("z-dir")).expect("创建 z 目录");
    for relative in ["z-dir/a.bin", "m.bin", "a.bin", "z.bin", "n.bin"] {
        fs::write(root.join(relative), relative.as_bytes()).expect("写入扫描文件");
    }

    let first = scan_managed_root_from_cursor(&root, "", 2).await;
    assert_eq!(scan_relatives(&root, &first), vec!["a.bin", "m.bin"]);
    assert!(first.has_more);
    let second = scan_managed_root_from_cursor(&root, &first.next_cursor, 2).await;
    assert_eq!(scan_relatives(&root, &second), vec!["n.bin", "z-dir/a.bin"]);
    assert!(second.has_more);
    let third = scan_managed_root_from_cursor(&root, &second.next_cursor, 2).await;
    assert_eq!(scan_relatives(&root, &third), vec!["z.bin"]);
    assert!(!third.has_more);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn scan_page_and_lookahead_are_bounded_by_limit_plus_one() {
    let root = test_root("scan-bounded-page");
    for index in (0..64).rev() {
        fs::write(root.join(format!("{index:03}.bin")), b"candidate").expect("写入有界扫描文件");
    }

    let page = scan_managed_root_from_cursor(&root, "", 3).await;
    assert_eq!(page.candidates.len(), 3);
    assert!(page.has_more);
    assert_eq!(
        scan_relatives(&root, &page),
        vec!["000.bin", "001.bin", "002.bin"]
    );
    let tail = scan_managed_root_from_cursor(&root, &page.next_cursor, 3).await;
    assert_eq!(tail.candidates.len(), 3);
    assert_eq!(
        scan_relatives(&root, &tail),
        vec!["003.bin", "004.bin", "005.bin"]
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn missing_managed_root_is_an_empty_scan() {
    let root = test_root("missing-root").join("attachments");
    let page = scan_managed_root_from_cursor(&root, "", 32).await;
    assert!(page.candidates.is_empty());
    assert!(!page.has_more);
    assert!(page.next_cursor.is_empty());
    assert!(!root.exists());
    let _ = fs::remove_dir_all(root.parent().expect("测试根目录父级"));
}

#[test]
fn temp_grace_is_fail_closed_for_clock_or_metadata_errors() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    let old = now - Duration::from_secs(61);
    let recent = now - Duration::from_secs(59);
    assert!(should_expire_temp(Ok(old), now, Duration::from_secs(60)));
    assert!(!should_expire_temp(
        Ok(recent),
        now,
        Duration::from_secs(60)
    ));
    assert!(!should_expire_temp(
        Err(std::io::Error::other("metadata unavailable")),
        now,
        Duration::from_secs(0)
    ));
    assert!(!should_expire_temp(
        Ok(now + Duration::from_secs(1)),
        now,
        Duration::from_secs(0)
    ));
}

#[tokio::test]
async fn sweep_removes_only_strict_unindexed_regular_files() {
    let root = test_root("sweep");
    let hash = "a".repeat(64);
    let symlink_hash = "c".repeat(64);
    let candidate = root.join(format!("{hash}.bin"));
    let adjacent = root.join(format!("{hash}extra.bin"));
    let outside = root.join("outside.bin");
    let recent_temp = root.join(".ingest-123e4567-e89b-12d3-a456-426614174000.tmp");
    let nested_dir = root.join("legacy");
    let nested_hash = "e".repeat(64);
    let nested_candidate = nested_dir.join(format!("{nested_hash}.bin"));
    fs::write(&candidate, b"orphan").expect("写入孤立文件");
    fs::write(&adjacent, b"keep").expect("写入相邻前缀文件");
    fs::write(&outside, b"outside").expect("写入外部目标");
    fs::write(&recent_temp, b"in progress").expect("写入近期临时文件");
    fs::create_dir_all(&nested_dir).expect("创建嵌套目录");
    fs::write(&nested_candidate, b"nested orphan").expect("写入嵌套孤立文件");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join(format!("{symlink_hash}.bin")))
        .expect("创建 symlink");

    let report = sweep_managed_root(
        &root,
        ManagedRootSweepOptions::new(
            RootKind::Attachment,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            SystemTime::now(),
            Duration::MAX,
            32,
        ),
    )
    .await;
    assert_eq!(report.removed, 2);
    assert!(!candidate.exists());
    assert!(!nested_candidate.exists());
    assert!(adjacent.exists());
    assert!(recent_temp.exists());
    #[cfg(unix)]
    assert!(root.join(format!("{symlink_hash}.bin")).exists());
    assert!(outside.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_managed_root_is_fail_closed() {
    let root = test_root("root-symlink");
    let outside = test_root("root-symlink-outside");
    let hash = "d".repeat(64);
    let candidate = outside.join(format!("{hash}.bin"));
    fs::write(&candidate, b"must keep").expect("写入外部候选文件");
    let managed_link = root.join("managed");
    std::os::unix::fs::symlink(&outside, &managed_link).expect("创建受管 root symlink");
    let report = sweep_managed_root(
        &managed_link,
        ManagedRootSweepOptions::new(
            RootKind::Attachment,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            SystemTime::now(),
            Duration::ZERO,
            32,
        ),
    )
    .await;
    assert_eq!(report.removed, 0);
    assert!(candidate.exists());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_managed_root_ancestor_is_fail_closed() {
    let root = test_root("root-ancestor-symlink");
    let outside = test_root("root-ancestor-symlink-outside");
    let hash = "e".repeat(64);
    let candidate = outside.join(format!("{hash}.bin"));
    fs::write(&candidate, b"must keep").expect("写入外部候选文件");
    let link = root.join("linked-base");
    std::os::unix::fs::symlink(&outside, &link).expect("创建受管 root 祖先 symlink");
    let managed_root = root
        .join("missing-before-parent")
        .join("..")
        .join("linked-base")
        .join("attachments");
    let report = sweep_managed_root(
        &managed_root,
        ManagedRootSweepOptions::new(
            RootKind::Attachment,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
            SystemTime::now(),
            Duration::ZERO,
            32,
        ),
    )
    .await;
    assert_eq!(report.removed, 0);
    assert!(candidate.exists());
    assert!(validate_indexed_path(&managed_root, &candidate.to_string_lossy()).is_err());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[test]
fn indexed_path_requires_managed_root_regular_file_or_missing_leaf() {
    let root = test_root("validate");
    let outside = test_root("validate-outside");
    let file = root.join("custom-name.dat");
    fs::write(&file, b"content").expect("写入受管文件");
    let (canonical, state) = validate_indexed_path(&root, &file.to_string_lossy())
        .expect("自定义文件路径应按 DB 精确路径处理");
    assert_eq!(canonical, fs::canonicalize(&file).expect("规范化文件"));
    assert_eq!(state, ManagedPathState::Present);

    let nested_dir = root.join("legacy");
    fs::create_dir_all(&nested_dir).expect("创建历史附件目录");
    let nested = nested_dir.join("custom-nested.dat");
    fs::write(&nested, b"nested").expect("写入嵌套受管文件");
    let (canonical, state) = validate_indexed_path(&root, &nested.to_string_lossy())
        .expect("受管 root 内嵌套文件应按 DB 精确路径处理");
    assert_eq!(
        canonical,
        fs::canonicalize(&nested).expect("规范化嵌套文件")
    );
    assert_eq!(state, ManagedPathState::Present);

    let lexical = nested_dir.join("..").join("custom-name.dat");
    let (canonical, state) = validate_indexed_path(&root, &lexical.to_string_lossy())
        .expect("受管 root 内安全词法别名应规范到同一文件");
    assert_eq!(canonical, fs::canonicalize(&file).expect("规范化词法别名"));
    assert_eq!(state, ManagedPathState::Present);

    let missing = root.join("missing.bin");
    let (_, state) = validate_indexed_path(&root, &missing.to_string_lossy()).expect("缺失叶子");
    assert_eq!(state, ManagedPathState::Missing);
    assert!(validate_indexed_path(&root, &outside.join("secret").to_string_lossy()).is_err());
    assert!(validate_indexed_path(&root, &format!("{}-adjacent", root.display())).is_err());

    #[cfg(unix)]
    {
        let link = root.join("link.bin");
        std::os::unix::fs::symlink(&outside, &link).expect("创建目录 symlink");
        assert!(validate_indexed_path(&root, &link.to_string_lossy()).is_err());
    }
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[tokio::test]
async fn derived_names_do_not_accept_adjacent_hash_prefixes() {
    let root = test_root("names");
    let hash = "b".repeat(64);
    let thumbnail = root.join(format!("{hash}_thumb.webp-extra"));
    let cache = root.join(format!("{hash}.json.bak"));
    fs::write(&thumbnail, b"keep").expect("写入相邻缩略图");
    fs::write(&cache, b"keep").expect("写入相邻缓存");
    for kind in [RootKind::Thumbnail, RootKind::MultimodalCache] {
        let report = sweep_managed_root(
            &root,
            ManagedRootSweepOptions::new(
                kind,
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                SystemTime::now(),
                Duration::MAX,
                32,
            ),
        )
        .await;
        assert_eq!(report.removed, 0);
    }
    assert!(thumbnail.exists());
    assert!(cache.exists());
    let _ = fs::remove_dir_all(root);
}
