use super::db::{
    IndexedAttachment, PathReferenceIndex, ATTACHMENT_GC_CURSOR_KEY, ATTACHMENT_GC_CYCLE_KEY,
};
use super::outbox::{retry_unlink_outbox, retry_unlink_outbox_with_index};
use super::paths::ManagedAttachmentRoots;
use super::{
    reclaim_orphaned_attachments_at_roots, reclaim_with_commit_failure_at_roots, AttachmentGcReport,
};
use crate::vcp_modules::chat_manager::Attachment;
use crate::vcp_modules::file_manager::{
    attachment_gc_gate, commit_registered_attachment, commit_registered_attachment_unlocked,
};
use crate::vcp_modules::message_repository::MessageRepository;
use crate::vcp_modules::topic_types::TopicKey;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::sqlite::SqlitePoolOptions;

fn test_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "vcp-attachment-gc-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(root.join("attachments")).expect("创建附件目录");
    fs::create_dir_all(root.join("thumbnails")).expect("创建缩略图目录");
    fs::create_dir_all(root.join("multimodal_cache")).expect("创建多模态目录");
    root
}

fn roots(root: &Path) -> ManagedAttachmentRoots {
    ManagedAttachmentRoots::new(
        root.join("attachments"),
        root.join("thumbnails"),
        root.join("multimodal_cache"),
    )
}

async fn pool(max_connections: u32) -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect("sqlite::memory:")
        .await
        .expect("打开测试数据库");
    for statement in [
        "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at INTEGER)",
        "CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at INTEGER)",
        "CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            deleted_at INTEGER, PRIMARY KEY (owner_type, owner_id, topic_id)
        )",
        "CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, deleted_at INTEGER,
            PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
        )",
        "CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, attachment_order INTEGER NOT NULL DEFAULT 0,
            display_name TEXT NOT NULL DEFAULT '', status TEXT, src TEXT, created_at INTEGER NOT NULL DEFAULT 0,
            deleted_at INTEGER
        )",
        "CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL DEFAULT '', size INTEGER NOT NULL DEFAULT 0,
            internal_path TEXT NOT NULL, extracted_text TEXT, image_frames TEXT,
            thumbnail_path TEXT, created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0
        )",
        "CREATE TABLE settings (
            key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL
        )",
        "CREATE TABLE attachment_gc_unlink_outbox (
            root_kind TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (root_kind, relative_path)
        )",
        "CREATE TABLE attachment_gc_unlink_live_references (
            root_kind TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            hash TEXT NOT NULL,
            PRIMARY KEY (root_kind, relative_path, hash)
        )",
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("创建测试表");
    }
    pool
}

async fn insert_message(pool: &sqlx::SqlitePool, owner: &str, topic: &str, message: &str) {
    sqlx::query("INSERT INTO agents(agent_id, deleted_at) VALUES (?, NULL)")
        .bind(owner)
        .execute(pool)
        .await
        .expect("插入测试 agent");
    sqlx::query(
        "INSERT INTO topics(owner_type, owner_id, topic_id, deleted_at)
         VALUES ('agent', ?, ?, NULL)",
    )
    .bind(owner)
    .bind(topic)
    .execute(pool)
    .await
    .expect("插入测试 topic");
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, deleted_at)
         VALUES ('agent', ?, ?, ?, NULL)",
    )
    .bind(owner)
    .bind(topic)
    .bind(message)
    .execute(pool)
    .await
    .expect("插入测试消息");
}

async fn insert_group_message(pool: &sqlx::SqlitePool, group: &str, topic: &str, message: &str) {
    sqlx::query("INSERT INTO groups(group_id, deleted_at) VALUES (?, NULL)")
        .bind(group)
        .execute(pool)
        .await
        .expect("插入测试 group");
    sqlx::query(
        "INSERT INTO topics(owner_type, owner_id, topic_id, deleted_at)
         VALUES ('group', ?, ?, NULL)",
    )
    .bind(group)
    .bind(topic)
    .execute(pool)
    .await
    .expect("插入测试 group topic");
    sqlx::query(
        "INSERT INTO messages(owner_type, owner_id, topic_id, msg_id, deleted_at)
         VALUES ('group', ?, ?, ?, NULL)",
    )
    .bind(group)
    .bind(topic)
    .bind(message)
    .execute(pool)
    .await
    .expect("插入测试 group 消息");
}

async fn insert_attachment_index(
    pool: &sqlx::SqlitePool,
    hash: &str,
    internal_path: &Path,
    thumbnail_path: Option<&Path>,
) {
    sqlx::query("INSERT INTO attachments(hash, internal_path, thumbnail_path) VALUES (?, ?, ?)")
        .bind(hash)
        .bind(internal_path.to_string_lossy().as_ref())
        .bind(thumbnail_path.map(|path| path.to_string_lossy().into_owned()))
        .execute(pool)
        .await
        .expect("插入附件索引");
}

async fn insert_relation(pool: &sqlx::SqlitePool, owner: &str, topic: &str, msg: &str, hash: &str) {
    sqlx::query(
        "INSERT INTO message_attachments(owner_type, owner_id, topic_id, msg_id, hash, status)
         VALUES ('agent', ?, ?, ?, ?, 'ready')",
    )
    .bind(owner)
    .bind(topic)
    .bind(msg)
    .bind(hash)
    .execute(pool)
    .await
    .expect("插入附件关系");
}

async fn attachment_count(pool: &sqlx::SqlitePool, hash: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash = ?")
        .bind(hash)
        .fetch_one(pool)
        .await
        .expect("读取附件索引数量")
}

async fn relation_total(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments")
        .fetch_one(pool)
        .await
        .expect("读取附件关系总数")
}

async fn outbox_total(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM attachment_gc_unlink_outbox")
        .fetch_one(pool)
        .await
        .expect("读取 unlink 债务总数")
}

async fn insert_unlink_debt(
    pool: &sqlx::SqlitePool,
    root_kind: &str,
    relative_path: &str,
    created_at: i64,
) {
    sqlx::query(
        "INSERT INTO attachment_gc_unlink_outbox(root_kind, relative_path, created_at)
         VALUES (?, ?, ?)",
    )
    .bind(root_kind)
    .bind(relative_path)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("插入 unlink 债务");
}

async fn retry_outbox_once(
    pool: &sqlx::SqlitePool,
    roots: &ManagedAttachmentRoots,
) -> super::outbox::UnlinkReport {
    let mut connection = pool.acquire().await.expect("获取 unlink 测试连接");
    retry_unlink_outbox(&mut connection, roots)
        .await
        .expect("重试 unlink 债务")
}

async fn register_orphan_index(pool: &sqlx::SqlitePool, hash: &str, path: &Path) {
    commit_registered_attachment(
        pool,
        hash,
        "application/octet-stream",
        fs::metadata(path).expect("读取附件文件大小").len(),
        path.to_string_lossy().as_ref(),
        1,
    )
    .await
    .expect("通过生产注册 helper 写入附件索引");
}

fn assert_report_is_bounded(report: AttachmentGcReport) {
    assert!(report.reclaimed <= 256);
    assert!(report.ghost_files <= 3 * 4096);
}

#[tokio::test]
async fn shared_hash_is_reclaimed_only_after_last_live_relation() {
    let root = test_root("shared");
    let pool = pool(1).await;
    let shared_hash = "a".repeat(64);
    let orphan_hash = "b".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let shared_thumb = root.join("thumbnails/shared-thumb.bin");
    let orphan_path = root.join("attachments/orphan-custom.data");
    fs::write(&shared_path, b"shared").expect("写入共享文件");
    fs::write(&shared_thumb, b"thumbnail").expect("写入共享缩略图");
    fs::write(&orphan_path, b"orphan").expect("写入孤立文件");
    insert_message(&pool, "agent-a", "topic-a", "message-a").await;
    insert_message(&pool, "agent-b", "topic-b", "message-b").await;
    insert_relation(&pool, "agent-a", "topic-a", "message-a", &shared_hash).await;
    insert_relation(&pool, "agent-b", "topic-b", "message-b", &shared_hash).await;
    insert_attachment_index(&pool, &shared_hash, &shared_path, Some(&shared_thumb)).await;
    insert_attachment_index(&pool, &orphan_hash, &orphan_path, None).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("首次 GC");
    assert_report_is_bounded(report);
    assert_eq!(attachment_count(&pool, &shared_hash).await, 1);
    assert!(shared_path.exists());
    assert!(shared_thumb.exists());
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert!(!orphan_path.exists());

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_id = 'agent-a' AND topic_id = 'topic-a'",
    )
    .execute(&pool)
    .await
    .expect("删除第一条关系");
    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("第二次 GC");
    assert!(shared_path.exists());
    assert_eq!(attachment_count(&pool, &shared_hash).await, 1);

    sqlx::query(
        "DELETE FROM message_attachments
         WHERE owner_id = 'agent-b' AND topic_id = 'topic-b'",
    )
    .execute(&pool)
    .await
    .expect("删除最后一条关系");
    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("第三次 GC");
    assert_eq!(attachment_count(&pool, &shared_hash).await, 0);
    assert!(!shared_path.exists());
    assert!(!shared_thumb.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn exact_index_path_is_used_and_unsafe_paths_fail_closed() {
    let root = test_root("paths");
    let pool = pool(1).await;
    let custom_hash = "c".repeat(64);
    let outside_hash = "d".repeat(64);
    let symlink_hash = "e".repeat(64);
    let ghost_hash = "f".repeat(64);
    let custom_path = root.join("attachments/custom-name-without-hash.bin");
    let outside = root.join("outside.bin");
    let symlink_target = root.join("symlink-target.bin");
    fs::write(&custom_path, b"custom").expect("写入自定义路径");
    fs::write(&outside, b"outside").expect("写入越界目标");
    fs::write(&symlink_target, b"symlink target").expect("写入 symlink 目标");
    let escaped = root.join("attachments-escape.bin");
    let ghost_path = root.join(format!("attachments/{ghost_hash}.bin"));
    fs::write(&ghost_path, b"ghost").expect("写入幽灵文件");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&symlink_target, root.join("attachments/symlink.bin"))
        .expect("创建附件 symlink");

    insert_attachment_index(&pool, &custom_hash, &custom_path, None).await;
    insert_attachment_index(&pool, &outside_hash, &outside, None).await;
    insert_attachment_index(
        &pool,
        &symlink_hash,
        &root.join("attachments/symlink.bin"),
        None,
    )
    .await;
    let escaped_path = root.join("../attachments-escape.bin");
    insert_attachment_index(&pool, &ghost_hash, &escaped_path, None).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("路径安全 GC");
    assert_report_is_bounded(report);
    assert_eq!(attachment_count(&pool, &custom_hash).await, 0);
    assert!(!custom_path.exists());
    assert_eq!(attachment_count(&pool, &outside_hash).await, 1);
    assert!(outside.exists());
    assert_eq!(attachment_count(&pool, &symlink_hash).await, 1);
    #[cfg(unix)]
    assert!(root.join("attachments/symlink.bin").exists());
    assert!(symlink_target.exists());
    assert_eq!(attachment_count(&pool, &ghost_hash).await, 1);
    assert!(ghost_path.exists());
    assert!(!escaped.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn desktop_only_and_legacy_empty_path_are_preserved() {
    let root = test_root("desktop-only");
    let pool = pool(1).await;
    let hash = "1".repeat(64);
    insert_message(&pool, "agent-desktop", "topic-desktop", "message-desktop").await;
    sqlx::query(
        "INSERT INTO message_attachments(owner_type, owner_id, topic_id, msg_id, hash, status)
         VALUES ('agent', 'agent-desktop', 'topic-desktop', 'message-desktop', ?, 'desktop_only')",
    )
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("插入 desktop_only 关系");
    sqlx::query(
        "INSERT INTO attachments(hash, internal_path, thumbnail_path) VALUES (?, '', NULL)",
    )
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("插入旧库空路径索引");

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("desktop_only GC");
    assert_eq!(report.retained, 1);
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    let relation: String =
        sqlx::query_scalar("SELECT status FROM message_attachments WHERE hash = ?")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .expect("读取 desktop_only 关系");
    assert_eq!(relation, "desktop_only");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn deleted_message_relations_are_scrubbed_before_reclaim() {
    let root = test_root("deleted");
    let pool = pool(1).await;
    let hash = "2".repeat(64);
    let path = root.join("attachments/deleted.bin");
    fs::write(&path, b"deleted").expect("写入删除文件");
    insert_message(&pool, "agent-deleted", "topic-deleted", "message-deleted").await;
    insert_relation(
        &pool,
        "agent-deleted",
        "topic-deleted",
        "message-deleted",
        &hash,
    )
    .await;
    insert_attachment_index(&pool, &hash, &path, None).await;
    sqlx::query(
        "UPDATE messages SET deleted_at = 1
         WHERE owner_id = 'agent-deleted' AND topic_id = 'topic-deleted'",
    )
    .execute(&pool)
    .await
    .expect("标记消息删除");

    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("删除消息后的 GC");
    let relations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments WHERE hash = ?")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .expect("读取关系数量");
    assert_eq!(relations, 0);
    assert_eq!(attachment_count(&pool, &hash).await, 0);
    assert!(!path.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn deleted_topic_agent_and_group_relations_are_scrubbed_before_reclaim() {
    let root = test_root("deleted-owners");
    let pool = pool(1).await;
    let topic_hash = "6".repeat(64);
    let agent_hash = "7".repeat(64);
    let group_hash = "8".repeat(64);
    let topic_path = root.join("attachments/deleted-topic.bin");
    let agent_path = root.join("attachments/deleted-agent.bin");
    let group_path = root.join("attachments/deleted-group.bin");
    for path in [&topic_path, &agent_path, &group_path] {
        fs::write(path, b"deleted").expect("写入删除附件");
    }

    insert_message(&pool, "agent-topic-delete", "topic-delete", "message-topic").await;
    insert_relation(
        &pool,
        "agent-topic-delete",
        "topic-delete",
        "message-topic",
        &topic_hash,
    )
    .await;
    insert_attachment_index(&pool, &topic_hash, &topic_path, None).await;
    sqlx::query(
        "UPDATE topics SET deleted_at = 1
         WHERE owner_type = 'agent' AND owner_id = 'agent-topic-delete'
           AND topic_id = 'topic-delete'",
    )
    .execute(&pool)
    .await
    .expect("标记 topic 删除");

    insert_message(&pool, "agent-owner-delete", "topic-owner", "message-owner").await;
    insert_relation(
        &pool,
        "agent-owner-delete",
        "topic-owner",
        "message-owner",
        &agent_hash,
    )
    .await;
    insert_attachment_index(&pool, &agent_hash, &agent_path, None).await;
    sqlx::query("UPDATE agents SET deleted_at = 1 WHERE agent_id = 'agent-owner-delete'")
        .execute(&pool)
        .await
        .expect("标记 agent 删除");

    insert_group_message(&pool, "group-delete", "topic-group", "message-group").await;
    sqlx::query(
        "INSERT INTO message_attachments(owner_type, owner_id, topic_id, msg_id, hash, status)
         VALUES ('group', 'group-delete', 'topic-group', 'message-group', ?, 'ready')",
    )
    .bind(&group_hash)
    .execute(&pool)
    .await
    .expect("插入 group 附件关系");
    insert_attachment_index(&pool, &group_hash, &group_path, None).await;
    sqlx::query("UPDATE groups SET deleted_at = 1 WHERE group_id = 'group-delete'")
        .execute(&pool)
        .await
        .expect("标记 group 删除");

    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("删除 owner 后的 GC");
    for (hash, path) in [
        (&topic_hash, &topic_path),
        (&agent_hash, &agent_path),
        (&group_hash, &group_path),
    ] {
        assert_eq!(attachment_count(&pool, hash).await, 0);
        assert!(!path.exists());
        let relation_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments WHERE hash = ?")
                .bind(hash)
                .fetch_one(&pool)
                .await
                .expect("读取删除 owner 关系");
        assert_eq!(relation_count, 0);
    }
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn dangling_message_relation_is_not_a_live_reference() {
    let root = test_root("dangling");
    let pool = pool(1).await;
    let hash = "9".repeat(64);
    let path = root.join("attachments/dangling.bin");
    fs::write(&path, b"dangling").expect("写入悬空附件");
    sqlx::query(
        "INSERT INTO message_attachments(owner_type, owner_id, topic_id, msg_id, hash, status)
         VALUES ('agent', 'missing-agent', 'missing-topic', 'missing-message', ?, 'ready')",
    )
    .bind(&hash)
    .execute(&pool)
    .await
    .expect("插入悬空附件关系");
    insert_attachment_index(&pool, &hash, &path, None).await;

    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("清理悬空关系");
    assert_eq!(attachment_count(&pool, &hash).await, 0);
    assert!(!path.exists());
    let relation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments WHERE hash = ?")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .expect("读取悬空关系");
    assert_eq!(relation_count, 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn concurrent_reference_committed_before_gc_is_retained() {
    let root = test_root("race");
    let pool = pool(2).await;
    let hash = "3".repeat(64);
    let path = root.join("attachments/raced.bin");
    fs::write(&path, b"raced").expect("写入竞争文件");
    insert_message(&pool, "agent-raced", "topic-raced", "message-raced").await;
    insert_attachment_index(&pool, &hash, &path, None).await;

    let mut writer = pool.acquire().await.expect("获取并发写连接");
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *writer)
        .await
        .expect("开启并发写事务");
    sqlx::query(
        "INSERT INTO message_attachments(owner_type, owner_id, topic_id, msg_id, hash, status)
         VALUES ('agent', 'agent-raced', 'topic-raced', 'message-raced', ?, 'ready')",
    )
    .bind(&hash)
    .execute(&mut *writer)
    .await
    .expect("写入待提交关系");

    let (gc_started_tx, gc_started_rx) = tokio::sync::oneshot::channel();
    let gc_pool = pool.clone();
    let gc_root = roots(&root);
    let gc = tokio::spawn(async move {
        let _ = gc_started_tx.send(());
        reclaim_orphaned_attachments_at_roots(&gc_pool, &gc_root).await
    });
    gc_started_rx.await.expect("等待 GC 进入竞争点");
    sqlx::query("COMMIT")
        .execute(&mut *writer)
        .await
        .expect("提交并发关系");
    let report = gc.await.expect("等待 GC").expect("竞争 GC");
    assert_report_is_bounded(report);
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    assert!(path.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn concurrent_registered_reference_cannot_reuse_deleted_file() {
    let root = test_root("registered-race");
    let pool = pool(2).await;
    let hash = "5".repeat(64);
    let path = root.join("attachments/recreated.bin");
    fs::write(&path, b"before gc").expect("写入初始文件");
    insert_message(
        &pool,
        "agent-registered-race",
        "topic-registered-race",
        "message-registered-race",
    )
    .await;
    commit_registered_attachment(
        &pool,
        &hash,
        "application/octet-stream",
        b"before gc".len() as u64,
        path.to_string_lossy().as_ref(),
        1,
    )
    .await
    .expect("注册初始附件");

    let writer_gate = attachment_gc_gate().read().await;
    let gc_pool = pool.clone();
    let gc_root = roots(&root);
    let (gc_started_tx, gc_started_rx) = tokio::sync::oneshot::channel();
    let gc = tokio::spawn(async move {
        let _ = gc_started_tx.send(());
        reclaim_orphaned_attachments_at_roots(&gc_pool, &gc_root).await
    });
    gc_started_rx.await.expect("等待 GC 进入闸门竞争");

    fs::write(&path, b"registered after gc").expect("发布并发注册文件");
    commit_registered_attachment_unlocked(
        &pool,
        &hash,
        "application/octet-stream",
        b"registered after gc".len() as u64,
        path.to_string_lossy().as_ref(),
        2,
        &writer_gate,
    )
    .await
    .expect("通过生产 helper 注册并发附件");
    let key = TopicKey::new("agent", "agent-registered-race", "topic-registered-race");
    let attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: path.to_string_lossy().into_owned(),
        name: "recreated.bin".to_string(),
        size: b"registered after gc".len() as u64,
        hash: Some(hash.clone()),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: path.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let mut tx = pool.begin().await.expect("开启并发关系事务");
    MessageRepository::upsert_attachments_for_message(
        &mut tx,
        &key,
        "message-registered-race",
        2,
        &[attachment],
        &writer_gate,
    )
    .await
    .expect("通过生产 helper 写入并发关系");
    tx.commit().await.expect("提交并发注册关系");
    drop(writer_gate);
    gc.await.expect("等待注册竞争 GC").expect("注册竞争 GC");
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    assert!(path.exists());
    let relation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_attachments WHERE hash = ? AND deleted_at IS NULL",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .expect("读取并发关系");
    assert_eq!(relation_count, 1);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn repeated_gc_is_idempotent_and_cleans_strict_ghosts() {
    let root = test_root("repeat");
    let pool = pool(1).await;
    let hash = "4".repeat(64);
    let ghost = root.join(format!("attachments/{hash}.dat"));
    fs::write(&ghost, b"ghost").expect("写入幽灵文件");
    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("首次幂等 GC");
    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("第二次幂等 GC");
    assert_eq!(first.ghost_files, 1);
    assert_eq!(second.ghost_files, 0);
    assert!(!ghost.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_uses_bounded_attachment_cursor_until_tail_converges() {
    let root = test_root("bounded-attachments");
    let pool = pool(1).await;
    let mut paths = Vec::new();
    for index in 1..=257_u16 {
        let hash = format!("{index:064x}");
        let path = root.join(format!("attachments/{hash}.bin"));
        fs::write(&path, b"orphan").expect("写入有界附件");
        register_orphan_index(&pool, &hash, &path).await;
        paths.push(path);
    }

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("首轮有界 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert!(first.cursor.is_some());
    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("尾页有界 GC");
    assert_eq!(second.reclaimed, 1);
    assert!(second.has_more, "跨页新 unlink 债务需等待下一完整周期");
    assert!(second.cursor.is_none());
    let mut third = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("下一完整周期有界 GC");
    for _ in 0..3 {
        if !third.has_more {
            break;
        }
        third = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
            .await
            .expect("继续下一完整周期有界 GC");
    }
    assert!(!third.has_more);
    assert!(paths.iter().all(|path| !path.exists()));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn cross_page_equivalent_live_path_cannot_unlink_shared_file() {
    let root = test_root("cross-page-equivalent-path");
    let pool = pool(1).await;
    let orphan_hash = "0".repeat(64);
    let live_hash = "f".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let legacy_path = root.join("attachments/legacy/../shared.bin");
    fs::create_dir_all(root.join("attachments/legacy")).expect("创建跨页路径别名目录");
    fs::write(&shared_path, b"shared").expect("写入跨页共享文件");

    insert_message(
        &pool,
        "agent-cross-page",
        "topic-cross-page",
        "message-cross-page",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page",
        "topic-cross-page",
        "message-cross-page",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &orphan_hash, &shared_path, None).await;
    for index in 1..=255_u32 {
        let hash = format!("{index:064x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }
    insert_attachment_index(&pool, &live_hash, &legacy_path, None).await;

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行跨页 GC 首页");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(shared_path.exists(), "跨页 live alias 必须保护物理文件");

    let mut report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行跨页 GC 尾页");
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(shared_path.exists(), "尾页引用检查不得删除共享物理文件");
    for _ in 0..4 {
        if !report.has_more {
            break;
        }
        report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
            .await
            .expect("继续执行跨页 GC 周期");
    }
    assert!(!report.has_more);
    assert!(shared_path.exists(), "GC 完成后活路径仍必须存在");
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn cross_page_low_hash_live_write_clears_old_unlink_debt() {
    let root = test_root("cross-page-low-hash-live-write");
    let pool = pool(1).await;
    let live_hash = "a".repeat(64);
    let tail_hash = "f".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let raw_alias_path = root.join("attachments/legacy/../shared.bin");
    fs::create_dir_all(root.join("attachments/legacy")).expect("创建合法别名目录");
    fs::write(&shared_path, b"shared").expect("写入页间竞态共享文件");
    insert_unlink_debt(&pool, "attachment", "shared.bin", 1000).await;

    insert_message(
        &pool,
        "agent-cross-page-tail-live",
        "topic-cross-page-tail-live",
        "message-cross-page-tail-live",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page-tail-live",
        "topic-cross-page-tail-live",
        "message-cross-page-tail-live",
        &tail_hash,
    )
    .await;
    insert_attachment_index(&pool, &tail_hash, &root.join("attachments/tail.bin"), None).await;
    for index in 1..=256_u32 {
        let hash = format!("b{index:063x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行低 hash 写入竞态首页 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(outbox_total(&pool).await, 1);

    insert_message(
        &pool,
        "agent-cross-page-low-live",
        "topic-cross-page-low-live",
        "message-cross-page-low-live",
    )
    .await;
    let gate = attachment_gc_gate().read().await;
    let attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: raw_alias_path.to_string_lossy().into_owned(),
        name: "shared.bin".to_string(),
        size: b"shared".len() as u64,
        hash: Some(live_hash.clone()),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: raw_alias_path.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let key = TopicKey::new(
        "agent",
        "agent-cross-page-low-live",
        "topic-cross-page-low-live",
    );
    let managed_roots = roots(&root);
    let mut tx = pool.begin().await.expect("开启低 hash 活引用事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut tx,
        &key,
        "message-cross-page-low-live",
        2,
        &[attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("写入低 hash 合法别名活引用");
    tx.commit().await.expect("提交低 hash 活引用事务");
    drop(gate);

    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(relation_total(&pool).await, 2);
    assert_eq!(outbox_total(&pool).await, 0, "写入活引用必须原子清除旧债务");

    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行低 hash 写入竞态尾页 GC");
    assert!(!second.has_more);
    assert!(
        shared_path.exists(),
        "尾页终检不得删除低 hash 活引用物理文件"
    );
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(attachment_count(&pool, &tail_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let live_relation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message_attachments
         WHERE hash = ? AND deleted_at IS NULL",
    )
    .bind(&live_hash)
    .fetch_one(&pool)
    .await
    .expect("读取低 hash 活关系");
    assert_eq!(live_relation_count, 1);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn cross_page_low_hash_live_write_clears_nested_same_named_root_debt() {
    let root = test_root("cross-page-low-hash-nested-root");
    let pool = pool(1).await;
    let live_hash = "a".repeat(64);
    let tail_hash = "f".repeat(64);
    let shared_path = root.join("attachments/nested/attachments/shared.bin");
    let raw_alias_path = root.join("attachments/legacy/../nested/attachments/shared.bin");
    fs::create_dir_all(root.join("attachments/legacy")).expect("创建合法别名目录");
    fs::create_dir_all(root.join("attachments/nested/attachments")).expect("创建嵌套同名 root");
    fs::write(&shared_path, b"nested shared").expect("写入嵌套同名 root 共享文件");
    insert_unlink_debt(&pool, "attachment", "nested/attachments/shared.bin", 1000).await;

    insert_message(
        &pool,
        "agent-cross-page-nested-tail-live",
        "topic-cross-page-nested-tail-live",
        "message-cross-page-nested-tail-live",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page-nested-tail-live",
        "topic-cross-page-nested-tail-live",
        "message-cross-page-nested-tail-live",
        &tail_hash,
    )
    .await;
    insert_attachment_index(&pool, &tail_hash, &root.join("attachments/tail.bin"), None).await;
    for index in 1..=256_u32 {
        let hash = format!("b{index:063x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行嵌套同名 root 竞态首页 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(outbox_total(&pool).await, 1);

    insert_message(
        &pool,
        "agent-cross-page-nested-low-live",
        "topic-cross-page-nested-low-live",
        "message-cross-page-nested-low-live",
    )
    .await;
    let gate = attachment_gc_gate().read().await;
    let attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: raw_alias_path.to_string_lossy().into_owned(),
        name: "nested-shared.bin".to_string(),
        size: b"nested shared".len() as u64,
        hash: Some(live_hash.clone()),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: raw_alias_path.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let key = TopicKey::new(
        "agent",
        "agent-cross-page-nested-low-live",
        "topic-cross-page-nested-low-live",
    );
    let managed_roots = roots(&root);
    let mut tx = pool.begin().await.expect("开启嵌套同名 root 活引用事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut tx,
        &key,
        "message-cross-page-nested-low-live",
        2,
        &[attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("写入嵌套同名 root 低 hash 活引用");
    tx.commit().await.expect("提交嵌套同名 root 活引用事务");
    drop(gate);

    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(
        outbox_total(&pool).await,
        0,
        "嵌套 root 活引用必须清除精确债务"
    );
    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行嵌套同名 root 竞态尾页 GC");
    assert!(!second.has_more);
    assert!(
        shared_path.exists(),
        "嵌套同名 root 尾页终检不得删除活引用文件"
    );
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(attachment_count(&pool, &tail_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_page_root_external_symlink_alias_keeps_shared_file() {
    let root = test_root("cross-page-root-external-alias");
    let outside = test_root("cross-page-root-external-alias-outside");
    let pool = pool(1).await;
    let orphan_hash = "a".repeat(64);
    let live_hash = "f".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let alias_path = outside.join("alias.bin");
    fs::write(&shared_path, b"shared").expect("写入跨页 root 外 alias 共享文件");
    std::os::unix::fs::symlink(&shared_path, &alias_path).expect("创建跨页 root 外 symlink alias");
    insert_message(
        &pool,
        "agent-cross-page-external-alias",
        "topic-cross-page-external-alias",
        "message-cross-page-external-alias",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page-external-alias",
        "topic-cross-page-external-alias",
        "message-cross-page-external-alias",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &orphan_hash, &shared_path, None).await;
    for index in 1..=255_u32 {
        let hash = format!("b{index:063x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }
    insert_attachment_index(&pool, &live_hash, &alias_path, None).await;

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行 root 外 alias 跨页首页 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(shared_path.exists(), "跨页 root 外 alias 必须保护物理文件");

    let mut report = first;
    for _ in 0..8 {
        assert!(shared_path.exists(), "跨页 GC 过程中共享文件必须持续存在");
        if !report.has_more {
            break;
        }
        report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
            .await
            .expect("继续执行 root 外 alias 跨页 GC");
    }
    assert!(!report.has_more, "跨页 GC 必须最终收敛");
    assert!(shared_path.exists(), "跨页 GC 完成后共享文件仍必须存在");
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_page_symlink_ancestor_keeps_shared_file() {
    let root = test_root("cross-page-symlink-ancestor");
    let pool = pool(1).await;
    let orphan_hash = "a".repeat(64);
    let live_hash = "f".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let alias_ancestor = root.join("attachments/alias-parent");
    let alias_path = alias_ancestor.join("shared.bin");
    fs::write(&shared_path, b"shared").expect("写入跨页 symlink ancestor 共享文件");
    std::os::unix::fs::symlink(root.join("attachments"), &alias_ancestor)
        .expect("创建跨页 symlink ancestor");
    insert_message(
        &pool,
        "agent-cross-page-symlink-ancestor",
        "topic-cross-page-symlink-ancestor",
        "message-cross-page-symlink-ancestor",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page-symlink-ancestor",
        "topic-cross-page-symlink-ancestor",
        "message-cross-page-symlink-ancestor",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &orphan_hash, &shared_path, None).await;
    for index in 1..=255_u32 {
        let hash = format!("b{index:063x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }
    insert_attachment_index(&pool, &live_hash, &alias_path, None).await;

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行 symlink ancestor 跨页首页 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(
        shared_path.exists(),
        "跨页 symlink ancestor 必须保护物理文件"
    );

    let mut report = first;
    for _ in 0..8 {
        assert!(shared_path.exists(), "跨页 GC 过程中共享文件必须持续存在");
        if !report.has_more {
            break;
        }
        report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
            .await
            .expect("继续执行 symlink ancestor 跨页 GC");
    }
    assert!(!report.has_more, "跨页 GC 必须最终收敛");
    assert!(shared_path.exists(), "跨页 GC 完成后共享文件仍必须存在");
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn stale_attachment_cursor_resets_before_consuming_old_unlink_debt() {
    let root = test_root("stale-attachment-cursor");
    let pool = pool(1).await;
    let live_hash = "a".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let raw_path = root.join("attachments/legacy/../shared.bin");
    fs::create_dir_all(root.join("attachments/legacy")).expect("创建 stale cursor 路径别名目录");
    fs::write(&shared_path, b"shared").expect("写入 stale cursor 共享文件");
    insert_message(
        &pool,
        "agent-stale-cursor",
        "topic-stale-cursor",
        "message-stale-cursor",
    )
    .await;
    insert_relation(
        &pool,
        "agent-stale-cursor",
        "topic-stale-cursor",
        "message-stale-cursor",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &live_hash, &raw_path, None).await;
    insert_unlink_debt(&pool, "attachment", "shared.bin", 1000).await;
    sqlx::query(
        "INSERT INTO settings(key, value, updated_at) VALUES (?, ?, 0)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(ATTACHMENT_GC_CURSOR_KEY)
    .bind("f".repeat(64))
    .execute(&pool)
    .await
    .expect("写入 stale GC 游标");
    sqlx::query(
        "INSERT INTO settings(key, value, updated_at) VALUES (?, ?, 0)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(ATTACHMENT_GC_CYCLE_KEY)
    .bind("2000")
    .execute(&pool)
    .await
    .expect("写入旧格式 GC 周期状态");

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("stale cursor 首轮必须安全 reset");
    assert!(first.has_more, "stale cursor reset 后必须要求从头继续扫描");
    assert!(first.cursor.is_none());
    assert!(
        shared_path.exists(),
        "stale cursor 首轮不得消费旧 unlink 债务"
    );
    assert_eq!(outbox_total(&pool).await, 1);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);

    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("stale cursor reset 后完整扫描");
    assert!(!second.has_more);
    assert!(shared_path.exists(), "从头完整扫描后活引用文件仍必须存在");
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert_eq!(outbox_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_uses_bounded_relation_cursor_until_tail_converges() {
    let root = test_root("bounded-relations");
    let pool = pool(1).await;
    for index in 0..257_u16 {
        insert_relation(
            &pool,
            &format!("missing-owner-{index}"),
            "missing-topic",
            &format!("missing-message-{index}"),
            &"a".repeat(64),
        )
        .await;
    }
    assert_eq!(relation_total(&pool).await, 257);
    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("首轮关系分页 GC");
    assert!(first.has_more);
    assert_eq!(relation_total(&pool).await, 1);
    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("尾页关系分页 GC");
    assert!(!second.has_more);
    assert_eq!(relation_total(&pool).await, 0);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_commit_failure_keeps_index_and_physical_file() {
    let root = test_root("commit-failure");
    let pool = pool(1).await;
    let hash = "b".repeat(64);
    let path = root.join("attachments/commit-failure.bin");
    fs::write(&path, b"keep").expect("写入提交失败附件");
    register_orphan_index(&pool, &hash, &path).await;

    let result = reclaim_with_commit_failure_at_roots(&pool, &roots(&root)).await;
    assert!(result.is_err());
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    assert!(path.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_preserves_cross_shared_main_and_thumbnail_paths_until_last_index() {
    let root = test_root("cross-shared-paths");
    let pool = pool(1).await;
    let first_hash = "c".repeat(64);
    let second_hash = "d".repeat(64);
    let main_path = root.join("attachments/shared-main.bin");
    let thumb_path = root.join("thumbnails/shared-thumb.bin");
    fs::write(&main_path, b"main").expect("写入共享主文件");
    fs::write(&thumb_path, b"thumbnail").expect("写入共享缩略图");
    register_orphan_index(&pool, &first_hash, &main_path).await;
    register_orphan_index(&pool, &second_hash, &main_path).await;
    sqlx::query("UPDATE attachments SET thumbnail_path = ? WHERE hash = ?")
        .bind(thumb_path.to_string_lossy().as_ref())
        .bind(&first_hash)
        .execute(&pool)
        .await
        .expect("设置第一条缩略图路径");
    sqlx::query("UPDATE attachments SET thumbnail_path = ? WHERE hash = ?")
        .bind(thumb_path.to_string_lossy().as_ref())
        .bind(&second_hash)
        .execute(&pool)
        .await
        .expect("设置第二条缩略图路径");

    reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("交叉共享路径 GC");
    assert_eq!(attachment_count(&pool, &first_hash).await, 0);
    assert_eq!(attachment_count(&pool, &second_hash).await, 0);
    assert!(!main_path.exists());
    assert!(!thumb_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_counts_equivalent_managed_paths_before_unlinking() {
    let root = test_root("equivalent-paths");
    let pool = pool(1).await;
    let orphan_hash = "f".repeat(64);
    let live_hash = "0".repeat(64);
    let canonical_path = root.join("attachments/shared.bin");
    let lexical_path = root.join("attachments/legacy/../shared.bin");
    fs::create_dir_all(root.join("attachments/legacy")).expect("创建词法别名目录");
    fs::write(&canonical_path, b"shared").expect("写入共享物理文件");
    insert_message(
        &pool,
        "agent-equivalent",
        "topic-equivalent",
        "message-equivalent",
    )
    .await;
    insert_relation(
        &pool,
        "agent-equivalent",
        "topic-equivalent",
        "message-equivalent",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &orphan_hash, &canonical_path, None).await;
    insert_attachment_index(&pool, &live_hash, &lexical_path, None).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("等价受管路径 GC");
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(canonical_path.exists(), "仍被活关系引用的物理文件不得删除");
    assert_eq!(report.retained, 1);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_preserves_paths_with_trailing_spaces_without_trimming() {
    let root = test_root("trailing-space-path");
    let pool = pool(1).await;
    let orphan_hash = "1".repeat(64);
    let live_hash = "2".repeat(64);
    let shared_path = root.join("attachments/shared-name.bin ");
    fs::write(&shared_path, b"shared").expect("写入带尾部空格的文件名");
    insert_message(
        &pool,
        "agent-trailing-space",
        "topic-trailing-space",
        "message-trailing-space",
    )
    .await;
    insert_relation(
        &pool,
        "agent-trailing-space",
        "topic-trailing-space",
        "message-trailing-space",
        &live_hash,
    )
    .await;
    insert_attachment_index(&pool, &orphan_hash, &shared_path, None).await;
    insert_attachment_index(&pool, &live_hash, &shared_path, None).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("带尾部空格路径 GC");
    assert_eq!(report.reclaimed, 1);
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &live_hash).await, 1);
    assert!(
        shared_path.exists(),
        "带尾部空格的共享文件不得因 trim 被删除"
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn unresolvable_nonempty_reference_aborts_physical_unlink() {
    let root = test_root("uncertain-reference");
    let pool = pool(1).await;
    let orphan_hash = "3".repeat(64);
    let uncertain_hash = "4".repeat(64);
    let orphan_path = root.join("attachments/orphan.bin");
    let uncertain_path = root.join("attachments/missing-parent/legacy.bin");
    fs::write(&orphan_path, b"must keep after rollback").expect("写入待保护附件");
    insert_attachment_index(&pool, &orphan_hash, &orphan_path, None).await;
    sqlx::query("INSERT INTO attachments(hash, internal_path, thumbnail_path) VALUES (?, ?, NULL)")
        .bind(&uncertain_hash)
        .bind(uncertain_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .expect("插入无法规范化的非空路径");

    let result = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root)).await;
    assert!(result.is_err(), "无法证明不相等的活引用必须 fail-closed");
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 1);
    assert!(orphan_path.exists());
    assert_eq!(attachment_count(&pool, &uncertain_hash).await, 1);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_alias_keeps_the_same_managed_physical_file() {
    let root = test_root("symlink-alias");
    let pool = pool(1).await;
    let orphan_hash = "5".repeat(64);
    let alias_hash = "6".repeat(64);
    let physical_path = root.join("attachments/physical.bin");
    let alias_path = root.join("attachments/alias.bin");
    fs::write(&physical_path, b"shared physical file").expect("写入受管物理文件");
    std::os::unix::fs::symlink(&physical_path, &alias_path).expect("创建受管 symlink alias");
    insert_attachment_index(&pool, &orphan_hash, &physical_path, None).await;
    insert_attachment_index(&pool, &alias_hash, &alias_path, None).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("受管 symlink alias GC");
    assert_eq!(report.reclaimed, 1);
    assert_eq!(attachment_count(&pool, &orphan_hash).await, 0);
    assert_eq!(attachment_count(&pool, &alias_hash).await, 1);
    assert!(
        physical_path.exists(),
        "symlink alias 引用的物理文件不得删除"
    );
    assert!(alias_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn root_external_symlink_alias_is_uncertain_and_blocks_unlink() {
    let root = test_root("external-symlink-alias");
    let outside = test_root("external-symlink-alias-outside");
    let pool = pool(1).await;
    let physical_hash = "7".repeat(64);
    let alias_hash = "8".repeat(64);
    let physical_path = root.join("attachments/physical.bin");
    let alias_path = outside.join("alias.bin");
    fs::write(&physical_path, b"externally aliased physical file").expect("写入受管物理文件");
    std::os::unix::fs::symlink(&physical_path, &alias_path).expect("创建 root 外 symlink alias");
    insert_attachment_index(&pool, &physical_hash, &physical_path, None).await;
    insert_attachment_index(&pool, &alias_hash, &alias_path, None).await;

    let result = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root)).await;
    assert!(
        result.is_err(),
        "root 外 symlink alias 必须按不确定引用处理"
    );
    assert_eq!(attachment_count(&pool, &physical_hash).await, 1);
    assert_eq!(attachment_count(&pool, &alias_hash).await, 1);
    assert!(
        physical_path.exists(),
        "不确定 alias 不得 unlink 受管物理文件"
    );
    assert!(alias_path.exists());
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[tokio::test]
async fn expired_temp_file_referenced_by_attachment_is_retained() {
    let root = test_root("referenced-expired-temp");
    let pool = pool(1).await;
    let hash = "9".repeat(64);
    let temp = root.join("attachments/.ingest-123e4567-e89b-12d3-a456-426614174000.tmp");
    fs::write(&temp, b"referenced temp").expect("写入受引用临时文件");
    insert_attachment_index(&pool, &hash, &temp, None).await;

    let roots = roots(&root);
    let mut connection = pool.acquire().await.expect("获取 GC 测试连接");
    let path_index = PathReferenceIndex::load_for_paths(
        &mut *connection,
        &roots,
        &[(super::paths::RootKind::Attachment, temp.clone())],
        &[],
    )
    .await
    .expect("加载附件路径快照");
    let report = super::paths::sweep_root_after_commit_for_test(
        &mut *connection,
        &roots.attachments,
        super::paths::RootKind::Attachment,
        &path_index,
        SystemTime::now(),
        Duration::ZERO,
    )
    .await
    .expect("扫描过期临时文件");
    assert_eq!(report.removed, 0);
    assert!(temp.exists(), "仍被附件索引引用的临时文件不得 unlink");
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn gc_treats_missing_empty_root_as_successful_scan() {
    let root = test_root("missing-empty-root");
    let attachments = root.join("attachments");
    fs::remove_dir(&attachments).expect("移除空附件 root");
    let pool = pool(1).await;

    let report = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("缺失空 root 不得阻断冷启动维护");
    assert!(!report.has_more);
    assert!(!attachments.exists(), "空 root 可保持缺失而作为空扫描");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn unlink_failure_keeps_debt_until_a_later_retry_succeeds() {
    let root = test_root("unlink-retry");
    let pool = pool(1).await;
    let blocked_dir = root.join("attachments/blocked");
    let blocked_file = blocked_dir.join("file.bin");
    let moved_dir = root.join("attachments/blocked-real");
    fs::create_dir_all(&blocked_dir).expect("创建 unlink 测试目录");
    fs::write(&blocked_file, b"retry").expect("写入 unlink 测试文件");
    insert_unlink_debt(&pool, "attachment", "blocked/file.bin", 1).await;
    fs::rename(&blocked_dir, &moved_dir).expect("暂存受管目录");
    fs::create_dir_all(root.join("outside")).expect("创建 symlink 目标目录");
    std::os::unix::fs::symlink(root.join("outside"), &blocked_dir).expect("创建阻断父目录 symlink");

    let first = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(first.deferred, 1);
    assert_eq!(outbox_total(&pool).await, 1);
    assert!(moved_dir.join("file.bin").exists());

    fs::remove_file(&blocked_dir).expect("移除阻断 symlink");
    fs::rename(&moved_dir, &blocked_dir).expect("恢复受管目录");
    let second = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(second.removed, 1);
    assert_eq!(second.deferred, 0);
    assert_eq!(outbox_total(&pool).await, 0);
    assert!(!blocked_file.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn re_referenced_debt_is_cleared_without_deleting_the_file() {
    let root = test_root("unlink-stale-reference");
    let pool = pool(1).await;
    let hash = "e".repeat(64);
    let path = root.join("attachments/referenced.bin");
    fs::write(&path, b"keep").expect("写入重新引用附件");
    insert_attachment_index(&pool, &hash, &path, None).await;
    insert_unlink_debt(&pool, "attachment", "referenced.bin", 1).await;

    let report = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(report.removed, 0);
    assert_eq!(report.deferred, 0);
    assert_eq!(outbox_total(&pool).await, 0);
    assert!(path.exists(), "重新引用的附件物理文件不得被误删");
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn unlink_outbox_preserves_trailing_space_in_relative_file_name() {
    let root = test_root("unlink-trailing-space");
    let pool = pool(1).await;
    let path = root.join("attachments/file.bin ");
    fs::write(&path, b"unlink me").expect("写入尾部空格 unlink 文件");
    insert_unlink_debt(&pool, "attachment", "file.bin ", 1).await;

    let report = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(report.removed, 1);
    assert_eq!(report.deferred, 0);
    assert_eq!(outbox_total(&pool).await, 0);
    assert!(!path.exists(), "unlink 相对路径不得 trim 尾部空格");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn unlink_cursor_consumes_tail_after_deferred_head_across_connections() {
    let root = test_root("unlink-cursor");
    let pool = pool(1).await;
    let blocked_dir = root.join("attachments/blocked");
    let blocked_file = blocked_dir.join("file.bin");
    let moved_dir = root.join("attachments/blocked-real");
    fs::create_dir_all(&blocked_dir).expect("创建队首延期目录");
    fs::write(&blocked_file, b"blocked").expect("写入队首延期文件");
    insert_unlink_debt(&pool, "attachment", "blocked/file.bin", 1).await;
    fs::rename(&blocked_dir, &moved_dir).expect("暂存队首延期目录");
    fs::create_dir_all(root.join("outside")).expect("创建队首 symlink 目标");
    std::os::unix::fs::symlink(root.join("outside"), &blocked_dir).expect("创建队首阻断 symlink");

    for index in 0..256 {
        let relative = format!("tail-{index:03}.bin");
        fs::write(root.join("attachments").join(&relative), b"tail").expect("写入尾部 unlink 文件");
        insert_unlink_debt(&pool, "attachment", &relative, 2).await;
    }

    let first = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(first.deferred, 1);
    assert_eq!(first.removed, 255);
    assert!(first.has_more, "首个延期债务不得饿死后续页");
    assert_eq!(outbox_total(&pool).await, 2);

    let second = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(second.removed, 1);
    assert!(!second.has_more);
    assert_eq!(outbox_total(&pool).await, 1);
    assert!(!root.join("attachments/tail-255.bin").exists());

    fs::remove_file(&blocked_dir).expect("移除队首阻断 symlink");
    fs::rename(&moved_dir, &blocked_dir).expect("恢复队首延期目录");
    let third = retry_outbox_once(&pool, &roots(&root)).await;
    assert_eq!(third.removed, 1);
    assert_eq!(third.deferred, 0);
    assert_eq!(outbox_total(&pool).await, 0);
    assert!(!blocked_file.exists());
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ghost_scan_cursor_advances_through_tail_and_resets_after_completion() {
    let root = test_root("scan-cursor");
    let pool = pool(1).await;
    for index in 1..=4097 {
        let hash = format!("{index:064x}");
        fs::write(
            root.join("attachments").join(format!("{hash}.bin")),
            b"ghost",
        )
        .expect("写入扫描游标幽灵文件");
    }

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("首轮扫描游标 GC");
    assert_eq!(first.ghost_files, 4096);
    assert!(first.has_more);
    let second = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("尾部扫描游标 GC");
    assert_eq!(second.ghost_files, 1);
    assert!(!second.has_more);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM settings
             WHERE key = 'maintenance.attachment_gc.scan_cursor.attachment'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        ""
    );
    let third = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("重置后的扫描游标 GC");
    assert_eq!(third.ghost_files, 0);
    assert!(!third.has_more);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn ghost_page_uses_one_loaded_snapshot_without_candidate_sql() {
    let root = test_root("snapshot-structure");
    let _pool = pool(1).await;
    let roots = roots(&root);
    let path_index = PathReferenceIndex::default();

    for index in 1..=4097_u32 {
        let hash = format!("{index:064x}");
        fs::write(
            root.join("attachments").join(format!("{hash}.bin")),
            b"ghost",
        )
        .expect("写入快照结构测试幽灵文件");
    }
    let report = super::paths::sweep_snapshot_page_for_test(
        &roots.attachments,
        super::paths::RootKind::Attachment,
        &path_index,
        4096,
    )
    .await
    .expect("使用已加载快照扫描幽灵页");
    assert_eq!(report.removed, 4096);
    assert_eq!(report.inspected, 4096);
    assert!(report.has_more);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn bounded_reference_probe_does_not_materialize_unrelated_large_tables() {
    let root = test_root("bounded-reference-probe");
    let pool = pool(1).await;
    let candidate_hash = "f".repeat(64);
    let candidate_path = root.join("attachments/candidate.bin");
    fs::write(&candidate_path, b"candidate").expect("写入候选附件");

    for index in 0..1024_u32 {
        let hash = format!("{index:064x}");
        sqlx::query(
            "INSERT INTO attachments(hash, internal_path, thumbnail_path)
             VALUES (?, ?, NULL)",
        )
        .bind(hash)
        .bind(format!("/unrelated/attachment-{index}.bin"))
        .execute(&pool)
        .await
        .expect("插入大附件索引表");
        sqlx::query(
            "INSERT INTO message_attachments
             (owner_type, owner_id, topic_id, msg_id, hash, status)
             VALUES ('unrelated', ?, 'topic', ?, ?, 'ready')",
        )
        .bind(format!("owner-{index}"))
        .bind(format!("message-{index}"))
        .bind(format!("{index:064x}"))
        .execute(&pool)
        .await
        .expect("插入大有效关系表");
    }
    insert_attachment_index(&pool, &candidate_hash, &candidate_path, None).await;

    let roots = roots(&root);
    let mut connection = pool.acquire().await.expect("获取引用探针连接");
    let index = PathReferenceIndex::load_for_records(
        &mut *connection,
        &roots,
        &[IndexedAttachment {
            hash: candidate_hash,
            internal_path: candidate_path.to_string_lossy().into_owned(),
            thumbnail_path: None,
        }],
    )
    .await
    .expect("加载有界引用探针");
    assert_eq!(index.loaded_key_counts(), (1, 1, 0));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn outbox_final_db_check_keeps_file_when_referenced_after_snapshot() {
    let root = test_root("outbox-final-db-check");
    let pool = pool(2).await;
    let hash = "a".repeat(64);
    let path = root.join("attachments/referenced-after-snapshot.bin");
    fs::write(&path, b"must keep").expect("写入 outbox 终检附件");
    insert_unlink_debt(&pool, "attachment", "referenced-after-snapshot.bin", 1).await;
    let roots = roots(&root);

    let mut snapshot_connection = pool.acquire().await.expect("获取 outbox 快照连接");
    let stale_index = PathReferenceIndex::load_for_paths(
        &mut *snapshot_connection,
        &roots,
        &[(super::paths::RootKind::Attachment, path.clone())],
        &[],
    )
    .await
    .expect("加载 outbox 旧快照");
    drop(snapshot_connection);

    insert_attachment_index(&pool, &hash, &path, None).await;
    let mut retry_connection = pool.acquire().await.expect("获取 outbox 重试连接");
    let report = retry_unlink_outbox_with_index(&mut *retry_connection, &roots, &stale_index)
        .await
        .expect("执行 outbox 终检");
    assert_eq!(report.removed, 0);
    assert_eq!(report.deferred, 0);
    assert_eq!(outbox_total(&pool).await, 0);
    assert!(path.exists(), "终检发现重引用时不得删除物理文件");
    assert_eq!(attachment_count(&pool, &hash).await, 1);
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn production_attachment_write_clears_only_exact_managed_root_debt() {
    let root = test_root("production-exact-root-debt");
    let pool = pool(1).await;
    let hash = "b".repeat(64);
    let shared_path = root.join("attachments/shared.bin");
    let nested_path = root.join("attachments/nested/attachments/shared.bin");
    fs::create_dir_all(nested_path.parent().expect("获取嵌套附件父目录"))
        .expect("创建嵌套同名附件目录");
    fs::write(&shared_path, b"outer shared").expect("写入外层共享文件");
    fs::write(&nested_path, b"nested shared").expect("写入嵌套共享文件");
    insert_unlink_debt(&pool, "attachment", "shared.bin", 1).await;
    insert_unlink_debt(&pool, "attachment", "nested/attachments/shared.bin", 1).await;
    insert_message(
        &pool,
        "agent-production-exact-root",
        "topic-production-exact-root",
        "message-production-exact-root",
    )
    .await;

    let managed_roots = roots(&root);
    let gate = attachment_gc_gate().read().await;
    let attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: nested_path.to_string_lossy().into_owned(),
        name: "shared.bin".to_string(),
        size: b"nested shared".len() as u64,
        hash: Some(hash),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: nested_path.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let key = TopicKey::new(
        "agent",
        "agent-production-exact-root",
        "topic-production-exact-root",
    );
    let mut tx = pool.begin().await.expect("开启生产附件写事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut tx,
        &key,
        "message-production-exact-root",
        2,
        &[attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("通过生产 roots-aware 附件入口写入活关系");
    tx.commit().await.expect("提交生产附件写事务");
    drop(gate);

    let debt_paths: Vec<String> = sqlx::query_scalar(
        "SELECT relative_path FROM attachment_gc_unlink_outbox
         WHERE root_kind = 'attachment' ORDER BY relative_path",
    )
    .fetch_all(&pool)
    .await
    .expect("读取精确 root unlink 债务");
    assert_eq!(debt_paths, vec!["shared.bin".to_string()]);
    assert!(shared_path.exists(), "同名外层债务对应文件必须保留");
    assert!(nested_path.exists(), "活引用对应文件必须保留");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[tokio::test]
async fn production_attachment_write_rejects_external_and_ancestor_symlink_aliases() {
    let root = test_root("production-symlink-alias");
    let outside = test_root("production-symlink-alias-outside");
    let pool = pool(1).await;
    let hash = "c".repeat(64);
    let managed_root = root.join("attachments");
    let shared_path = managed_root.join("shared.bin");
    let external_alias = outside.join("external.bin");
    let ancestor = managed_root.join("alias-parent");
    let ancestor_alias = ancestor.join("shared.bin");
    fs::write(&shared_path, b"shared symlink target").expect("写入 symlink 共享文件");
    std::os::unix::fs::symlink(&shared_path, &external_alias).expect("创建 root 外 symlink alias");
    std::os::unix::fs::symlink(&managed_root, &ancestor)
        .expect("创建受管 root 内祖先 symlink alias");
    insert_unlink_debt(&pool, "attachment", "shared.bin", 1).await;

    insert_message(
        &pool,
        "agent-production-external-alias",
        "topic-production-external-alias",
        "message-production-external-alias",
    )
    .await;
    insert_message(
        &pool,
        "agent-production-ancestor-alias",
        "topic-production-ancestor-alias",
        "message-production-ancestor-alias",
    )
    .await;

    let managed_roots = roots(&root);
    let gate = attachment_gc_gate().read().await;
    let external_attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: external_alias.to_string_lossy().into_owned(),
        name: "external.bin".to_string(),
        size: b"shared symlink target".len() as u64,
        hash: Some(hash.clone()),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: external_alias.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let external_key = TopicKey::new(
        "agent",
        "agent-production-external-alias",
        "topic-production-external-alias",
    );
    let mut external_tx = pool.begin().await.expect("开启 root 外 alias 写事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut external_tx,
        &external_key,
        "message-production-external-alias",
        2,
        &[external_attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("通过生产入口写入 root 外 alias 活关系");
    external_tx
        .commit()
        .await
        .expect("提交 root 外 alias 写事务");

    let ancestor_attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: ancestor_alias.to_string_lossy().into_owned(),
        name: "ancestor.bin".to_string(),
        size: b"shared symlink target".len() as u64,
        hash: Some(hash),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: ancestor_alias.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let ancestor_key = TopicKey::new(
        "agent",
        "agent-production-ancestor-alias",
        "topic-production-ancestor-alias",
    );
    let mut ancestor_tx = pool.begin().await.expect("开启祖先 alias 写事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut ancestor_tx,
        &ancestor_key,
        "message-production-ancestor-alias",
        3,
        &[ancestor_attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("通过生产入口写入祖先 alias 活关系");
    ancestor_tx.commit().await.expect("提交祖先 alias 写事务");
    drop(gate);

    let debt_paths: Vec<String> = sqlx::query_scalar(
        "SELECT relative_path FROM attachment_gc_unlink_outbox
         WHERE root_kind = 'attachment' ORDER BY relative_path",
    )
    .fetch_all(&pool)
    .await
    .expect("读取 symlink alias unlink 债务");
    assert_eq!(debt_paths, vec!["shared.bin".to_string()]);
    assert!(
        shared_path.exists(),
        "symlink alias 不得清除真实 root 文件债务"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_page_production_symlink_aliases_protect_only_their_exact_debts() {
    let root = test_root("cross-page-production-symlink-witness");
    let outside = test_root("cross-page-production-symlink-witness-outside");
    let pool = pool(1).await;
    let external_hash = "a".repeat(64);
    let ancestor_hash = "a1".to_string() + &"1".repeat(62);
    let tail_hash = "f".repeat(64);
    let custom_path = root.join("attachments/custom.bin");
    let nested_custom_path = root.join("attachments/nested/custom.bin");
    let unrelated_path = root.join("attachments/other/custom.bin");
    let external_alias = outside.join("custom.bin");
    let ancestor_dir = root.join("attachments/legacy");
    let ancestor_alias = ancestor_dir.join("nested/custom.bin");
    let thumbnail_path = root.join("thumbnails/preview.webp");
    let thumbnail_alias = outside.join("preview.webp");
    fs::create_dir_all(nested_custom_path.parent().expect("获取嵌套附件目录"))
        .expect("创建嵌套附件目录");
    fs::create_dir_all(unrelated_path.parent().expect("获取无关附件目录"))
        .expect("创建无关附件目录");
    fs::write(&custom_path, b"custom target").expect("写入 root 外 alias 目标");
    fs::write(&nested_custom_path, b"ancestor target").expect("写入祖先 alias 目标");
    fs::write(&unrelated_path, b"unrelated same name").expect("写入无关同名文件");
    fs::write(&thumbnail_path, b"thumbnail target").expect("写入缩略图目标");
    std::os::unix::fs::symlink(&custom_path, &external_alias).expect("创建 root 外附件 alias");
    std::os::unix::fs::symlink(&root.join("attachments"), &ancestor_dir)
        .expect("创建祖先附件 symlink");
    std::os::unix::fs::symlink(&thumbnail_path, &thumbnail_alias)
        .expect("创建 root 外缩略图 alias");

    insert_unlink_debt(&pool, "attachment", "custom.bin", 1000).await;
    insert_unlink_debt(&pool, "attachment", "nested/custom.bin", 1000).await;
    insert_unlink_debt(&pool, "attachment", "other/custom.bin", 1000).await;
    insert_unlink_debt(&pool, "thumbnail", "preview.webp", 1000).await;
    insert_message(
        &pool,
        "agent-cross-page-symlink-tail",
        "topic-cross-page-symlink-tail",
        "message-cross-page-symlink-tail",
    )
    .await;
    insert_relation(
        &pool,
        "agent-cross-page-symlink-tail",
        "topic-cross-page-symlink-tail",
        "message-cross-page-symlink-tail",
        &tail_hash,
    )
    .await;
    insert_attachment_index(&pool, &tail_hash, &root.join("attachments/tail.bin"), None).await;
    for index in 1..=256_u32 {
        let hash = format!("b{index:063x}");
        insert_attachment_index(
            &pool,
            &hash,
            &root.join("attachments").join(format!("{hash}.bin")),
            None,
        )
        .await;
    }

    let first = reclaim_orphaned_attachments_at_roots(&pool, &roots(&root))
        .await
        .expect("执行跨页 symlink witness 首页 GC");
    assert_eq!(first.reclaimed, 256);
    assert!(first.has_more);
    assert_eq!(outbox_total(&pool).await, 4);

    insert_message(
        &pool,
        "agent-cross-page-production-external",
        "topic-cross-page-production-external",
        "message-cross-page-production-external",
    )
    .await;
    insert_message(
        &pool,
        "agent-cross-page-production-ancestor",
        "topic-cross-page-production-ancestor",
        "message-cross-page-production-ancestor",
    )
    .await;
    let gate = attachment_gc_gate().read().await;
    let external_attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: external_alias.to_string_lossy().into_owned(),
        name: "custom.bin".to_string(),
        size: b"custom target".len() as u64,
        hash: Some(external_hash.clone()),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: external_alias.to_string_lossy().into_owned(),
        thumbnail_path: Some(thumbnail_alias.to_string_lossy().into_owned()),
        ..Attachment::default()
    };
    let external_key = TopicKey::new(
        "agent",
        "agent-cross-page-production-external",
        "topic-cross-page-production-external",
    );
    let managed_roots = roots(&root);
    let mut external_tx = pool.begin().await.expect("开启 root 外 alias 写事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut external_tx,
        &external_key,
        "message-cross-page-production-external",
        2,
        &[external_attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("通过生产入口写入 root 外附件和缩略图 alias");
    external_tx
        .commit()
        .await
        .expect("提交 root 外 alias 写事务");

    let ancestor_attachment = Attachment {
        r#type: "application/octet-stream".to_string(),
        src: ancestor_alias.to_string_lossy().into_owned(),
        name: "nested-custom.bin".to_string(),
        size: b"ancestor target".len() as u64,
        hash: Some(ancestor_hash),
        status: Some("ready".to_string()),
        attachment_order: Some(0),
        internal_path: ancestor_alias.to_string_lossy().into_owned(),
        ..Attachment::default()
    };
    let ancestor_key = TopicKey::new(
        "agent",
        "agent-cross-page-production-ancestor",
        "topic-cross-page-production-ancestor",
    );
    let mut ancestor_tx = pool.begin().await.expect("开启祖先 alias 写事务");
    MessageRepository::upsert_attachments_for_message_with_roots(
        &mut ancestor_tx,
        &ancestor_key,
        "message-cross-page-production-ancestor",
        3,
        &[ancestor_attachment],
        &gate,
        Some(&managed_roots),
    )
    .await
    .expect("通过生产入口写入祖先 symlink alias");
    ancestor_tx.commit().await.expect("提交祖先 alias 写事务");
    drop(gate);

    let witness_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM attachment_gc_unlink_live_references")
            .fetch_one(&pool)
            .await
            .expect("读取 symlink witness 数量");
    assert_eq!(witness_count, 3, "附件、祖先 alias 和缩略图各需一条见证");
    assert_eq!(outbox_total(&pool).await, 4, "写侧必须保留 alias 债务");

    let mut report = reclaim_orphaned_attachments_at_roots(&pool, &managed_roots)
        .await
        .expect("执行跨页 symlink witness 尾页 GC");
    for _ in 0..8 {
        if !report.has_more {
            break;
        }
        report = reclaim_orphaned_attachments_at_roots(&pool, &managed_roots)
            .await
            .expect("继续执行跨页 symlink witness GC");
    }
    assert!(!report.has_more, "有界 GC 应在有限页内收敛");
    assert!(custom_path.exists(), "root 外 alias 目标不得被终检删除");
    assert!(
        nested_custom_path.exists(),
        "祖先 symlink alias 目标不得被终检删除"
    );
    assert!(
        thumbnail_path.exists(),
        "root 外缩略图 alias 目标不得被终检删除"
    );
    assert!(!unrelated_path.exists(), "无关同名债务必须仍可安全 unlink");
    assert_eq!(
        outbox_total(&pool).await,
        0,
        "活引用债务应被安全清除而非物理误删"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(outside);
}
