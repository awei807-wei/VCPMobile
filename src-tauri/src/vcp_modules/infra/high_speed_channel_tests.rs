use super::{finalize_high_speed_upload, UploadMetadata};
use crate::vcp_modules::file_manager::{attachment_gc_gate, get_attachments_root_dir};
use crate::vcp_modules::infra::utils::calculate_sha256;
use sqlx::sqlite::SqlitePoolOptions;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

struct XdgConfigGuard {
    previous_config_home: Option<OsString>,
    root: PathBuf,
}

impl XdgConfigGuard {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("vcp-high-speed-xdg-{}", uuid::Uuid::new_v4()));
        let config_root = root.join("config");
        let documents = root.join("documents");
        fs::create_dir_all(&config_root).expect("创建高速上传 XDG 配置目录");
        fs::create_dir_all(&documents).expect("创建高速上传文档目录");
        fs::write(
            config_root.join("user-dirs.dirs"),
            format!("XDG_DOCUMENTS_DIR=\"{}\"\n", documents.display()),
        )
        .expect("写入高速上传 XDG 配置");
        let previous_config_home = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config_root);
        Self {
            previous_config_home,
            root,
        }
    }
}

impl Drop for XdgConfigGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous_config_home.take() {
            std::env::set_var("XDG_CONFIG_HOME", previous);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn registration_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("打开高速上传测试数据库");
    sqlx::raw_sql(
        "CREATE TABLE agents (agent_id TEXT PRIMARY KEY, deleted_at INTEGER);
         CREATE TABLE groups (group_id TEXT PRIMARY KEY, deleted_at INTEGER);
         CREATE TABLE topics (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            deleted_at INTEGER, PRIMARY KEY(owner_type, owner_id, topic_id)
         );
         CREATE TABLE messages (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, deleted_at INTEGER,
            PRIMARY KEY(owner_type, owner_id, topic_id, msg_id)
         );
         CREATE TABLE message_attachments (
            owner_type TEXT NOT NULL, owner_id TEXT NOT NULL, topic_id TEXT NOT NULL,
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, status TEXT, src TEXT, deleted_at INTEGER
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL, size INTEGER NOT NULL,
            internal_path TEXT NOT NULL, extracted_text TEXT, image_frames TEXT,
            thumbnail_path TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );",
    )
    .execute(&pool)
    .await
    .expect("创建高速上传测试表");
    pool
}

#[tokio::test]
async fn high_speed_finalizer_holds_gate_through_publish_and_registration() {
    // `tauri::test::mock_app()` resolves document_dir through the process-wide
    // XDG configuration. Registration tests temporarily replace that config and
    // remove it on drop, so serialize this fixture with the shared guard before
    // resolving any app paths.
    let _xdg_config = crate::vcp_modules::file_manager::lock_attachment_test_environment().await;
    let _xdg_root = XdgConfigGuard::new();
    let app = tauri::test::mock_app();
    let pool = registration_pool().await;
    let content = format!("高速上传 barrier {}", uuid::Uuid::new_v4()).into_bytes();
    let hash = calculate_sha256(&content);
    let root = get_attachments_root_dir(app.handle()).expect("解析附件根目录");
    fs::create_dir_all(&root).expect("创建附件根目录");
    let temp_path = root.join(format!(".ingest-{hash}-test.tmp"));
    let destination = root.join(format!("{hash}.bin"));
    fs::write(&temp_path, &content).expect("写入高速上传临时文件");
    let metadata = UploadMetadata {
        name: "barrier.bin".to_string(),
        mime: "application/octet-stream".to_string(),
        size: content.len() as u64,
    };
    let expected_size = content.len() as u64;
    let gate = attachment_gc_gate().write().await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let app_handle = app.handle().clone();
    let worker_pool = pool.clone();
    let worker_temp_path = temp_path.clone();
    let task = tokio::spawn(async move {
        let _ = started_tx.send(());
        finalize_high_speed_upload(
            &app_handle,
            &worker_pool,
            &worker_temp_path,
            &metadata,
            hash,
            expected_size,
        )
        .await
    });
    started_rx.await.expect("高速上传任务已进入闸门等待");
    assert!(!task.is_finished(), "写闸门持有时终结路径不得提前完成");
    drop(gate);
    let data = task
        .await
        .expect("等待高速上传终结任务")
        .expect("高速上传终结");
    assert_eq!(data.internal_path, destination.to_string_lossy());
    assert!(destination.is_file());
    let indexed: (String, i64) =
        sqlx::query_as("SELECT internal_path, size FROM attachments WHERE hash = ?")
            .bind(&data.hash)
            .fetch_one(&pool)
            .await
            .expect("读取高速上传索引");
    assert_eq!(indexed.0, destination.to_string_lossy());
    assert_eq!(indexed.1, expected_size as i64);
    let _ = fs::remove_file(&destination);
    let _ = fs::remove_file(&temp_path);
}
