use super::super::{
    attachment_gc_gate, commit_registered_attachment, commit_registered_attachment_unlocked,
    get_attachments_root_dir, register_attachment_internal_unlocked, AttachmentRegistrationInput,
};
use crate::vcp_modules::infra::utils::calculate_sha256;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

async fn relation(pool: &sqlx::SqlitePool, msg_id: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, src FROM message_attachments WHERE msg_id = ?")
        .bind(msg_id)
        .fetch_one(pool)
        .await
        .expect("read attachment relation")
}

async fn registration_pool() -> sqlx::SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open registration database");
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
            msg_id TEXT NOT NULL, hash TEXT NOT NULL, status TEXT, src TEXT,
            deleted_at INTEGER
         );
         CREATE TABLE attachments (
            hash TEXT PRIMARY KEY, mime_type TEXT NOT NULL DEFAULT '',
            size INTEGER NOT NULL DEFAULT 0, internal_path TEXT NOT NULL,
            extracted_text TEXT, thumbnail_path TEXT,
            created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0
         );",
    )
    .execute(&pool)
    .await
    .expect("create registration tables");
    pool
}

async fn seed_live_relation_fixture(pool: &sqlx::SqlitePool) {
    sqlx::raw_sql(
        "INSERT INTO agents VALUES ('live-agent', NULL), ('dead-agent', 1);
         INSERT INTO groups VALUES ('live-group', NULL);
         INSERT INTO topics (owner_type, owner_id, topic_id, deleted_at) VALUES
            ('agent', 'live-agent', 'live-agent-topic', NULL),
            ('group', 'live-group', 'live-group-topic', NULL),
            ('agent', 'live-agent', 'dead-topic', 1),
            ('agent', 'dead-agent', 'dead-owner-topic', NULL);
         INSERT INTO messages (owner_type, owner_id, topic_id, msg_id, deleted_at) VALUES
            ('agent', 'live-agent', 'live-agent-topic', 'live-agent-message', NULL),
            ('group', 'live-group', 'live-group-topic', 'live-group-message', NULL),
            ('agent', 'live-agent', 'dead-topic', 'dead-topic-message', NULL),
            ('agent', 'dead-agent', 'dead-owner-topic', 'dead-owner-message', NULL),
            ('agent', 'live-agent', 'live-agent-topic', 'dead-message', 1);
         INSERT INTO message_attachments
            (owner_type, owner_id, topic_id, msg_id, hash, status, src, deleted_at) VALUES
            ('agent', 'live-agent', 'live-agent-topic', 'live-agent-message', 'hash', 'desktop_only', NULL, NULL),
            ('group', 'live-group', 'live-group-topic', 'live-group-message', 'hash', 'desktop_only', NULL, NULL),
            ('agent', 'live-agent', 'dead-topic', 'dead-topic-message', 'hash', 'desktop_only', NULL, NULL),
            ('agent', 'dead-agent', 'dead-owner-topic', 'dead-owner-message', 'hash', 'desktop_only', NULL, NULL),
            ('agent', 'live-agent', 'live-agent-topic', 'dead-message', 'hash', 'desktop_only', NULL, NULL);",
    )
    .execute(pool)
    .await
    .expect("seed live relation fixture");
}

async fn assert_promoted_relations(pool: &sqlx::SqlitePool) {
    for message_id in ["live-agent-message", "live-group-message"] {
        assert_eq!(
            relation(pool, message_id).await,
            ("ready".to_string(), Some("file:///cas/hash".to_string()))
        );
    }
    for message_id in ["dead-topic-message", "dead-owner-message", "dead-message"] {
        assert_eq!(
            relation(pool, message_id).await,
            ("desktop_only".to_string(), None)
        );
    }
}

struct XdgConfigGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    previous_config_home: Option<OsString>,
    root: PathBuf,
}

impl XdgConfigGuard {
    async fn new() -> Self {
        let lock = super::lock_xdg_config_home().await;
        let root = super::test_root("registration-app");
        let config_root = root.join("config");
        let documents = root.join("documents");
        fs::create_dir_all(&config_root).expect("create XDG config root");
        fs::create_dir_all(&documents).expect("create XDG documents root");
        fs::write(
            config_root.join("user-dirs.dirs"),
            format!("XDG_DOCUMENTS_DIR=\"{}\"\n", documents.display()),
        )
        .expect("write XDG user directories");
        let previous_config_home = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &config_root);
        Self {
            _lock: lock,
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

struct RegistrationApp {
    app: tauri::App<tauri::test::MockRuntime>,
    root: PathBuf,
    _xdg: XdgConfigGuard,
}

async fn registration_app() -> RegistrationApp {
    let xdg = XdgConfigGuard::new().await;
    let app = tauri::test::mock_app();
    let root = get_attachments_root_dir(app.handle()).expect("resolve attachment root");
    fs::create_dir_all(&root).expect("create attachment root");
    RegistrationApp {
        app,
        root,
        _xdg: xdg,
    }
}

async fn registered_attachment_count(pool: &sqlx::SqlitePool, hash: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM attachments WHERE hash = ?")
        .bind(hash)
        .fetch_one(pool)
        .await
        .expect("read registered attachment count")
}

async fn insert_existing_cas(
    pool: &sqlx::SqlitePool,
    hash: &str,
    path: &std::path::Path,
    size: u64,
) {
    sqlx::query(
        "INSERT INTO attachments
            (hash, mime_type, size, internal_path, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(hash)
    .bind("application/pdf")
    .bind(size as i64)
    .bind(path.to_string_lossy().as_ref())
    .bind(123_i64)
    .bind(124_i64)
    .execute(pool)
    .await
    .expect("insert existing CAS metadata");
}

async fn attachment_metadata(pool: &sqlx::SqlitePool, hash: &str) -> (String, String, i64, i64) {
    sqlx::query_as(
        "SELECT mime_type, internal_path, size, created_at FROM attachments WHERE hash = ?",
    )
    .bind(hash)
    .fetch_one(pool)
    .await
    .expect("read CAS metadata")
}

fn spawn_queued_writer() -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (queued_tx, queued_rx) = tokio::sync::oneshot::channel();
    let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let _ = started_tx.send(());
        let write = attachment_gc_gate().write();
        tokio::pin!(write);
        loop {
            tokio::select! {
                gate = &mut write => {
                    let _ = acquired_tx.send(());
                    drop(gate);
                    return;
                }
                _ = tokio::task::yield_now() => {
                    if attachment_gc_gate().try_read().is_err() {
                        let _ = queued_tx.send(());
                        let gate = write.await;
                        let _ = acquired_tx.send(());
                        drop(gate);
                        return;
                    }
                }
            }
        }
    });
    (writer, started_rx, queued_rx, acquired_rx)
}

#[tokio::test]
async fn registration_promotes_only_live_message_topic_and_owner_relations() {
    let pool = registration_pool().await;
    seed_live_relation_fixture(&pool).await;
    commit_registered_attachment(&pool, "hash", "text/plain", 4, "/cas/hash", 10)
        .await
        .expect("register attachment");
    assert_promoted_relations(&pool).await;
}

#[tokio::test]
async fn unlocked_registration_commits_new_cas_before_best_effort_derivation() {
    let fixture = registration_app().await;
    let pool = registration_pool().await;
    let content = b"new CAS registration";
    let hash = calculate_sha256(content);
    let path = fixture.root.join(format!("{hash}.bin"));
    fs::write(&path, content).expect("write new CAS");
    let gate = attachment_gc_gate().read().await;
    let data = register_attachment_internal_unlocked(
        fixture.app.handle(),
        &pool,
        AttachmentRegistrationInput::new(
            hash.clone(),
            "new.bin".to_string(),
            "application/octet-stream".to_string(),
            content.len() as u64,
            path.to_string_lossy().into_owned(),
        ),
        &gate,
    )
    .await
    .expect("register new CAS");
    assert_eq!(data.hash, hash);
    assert_eq!(data.name, "new.bin");
    assert_eq!(data.mime_type, "application/octet-stream");
    assert_eq!(data.size, content.len() as u64);
    assert_eq!(data.extracted_text, None);
    assert_eq!(registered_attachment_count(&pool, &data.hash).await, 1);
    drop(gate);
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn derived_metadata_failure_keeps_committed_cas_and_empty_derivation() {
    let fixture = registration_app().await;
    let pool = registration_pool().await;
    let content = b"derived metadata failure";
    let hash = calculate_sha256(content);
    let path = fixture.root.join(format!("{hash}.txt"));
    fs::write(&path, content).expect("write derivation fixture");
    sqlx::query(
        "CREATE TRIGGER fail_attachment_derivation
         BEFORE UPDATE OF extracted_text ON attachments
         BEGIN SELECT RAISE(ABORT, 'injected derivation failure'); END",
    )
    .execute(&pool)
    .await
    .expect("create derivation failure trigger");

    let gate = attachment_gc_gate().read().await;
    let data = register_attachment_internal_unlocked(
        fixture.app.handle(),
        &pool,
        AttachmentRegistrationInput::new(
            hash.clone(),
            "derivation.txt".to_string(),
            "text/plain".to_string(),
            content.len() as u64,
            path.to_string_lossy().into_owned(),
        ),
        &gate,
    )
    .await
    .expect("core CAS registration should survive derivation failure");
    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT internal_path, extracted_text, thumbnail_path
         FROM attachments WHERE hash = ?",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .expect("read committed attachment");

    assert_eq!(data.hash, hash);
    assert_eq!(registered_attachment_count(&pool, &hash).await, 1);
    assert_eq!(row, (path.to_string_lossy().into_owned(), None, None));
    assert!(path.exists());
    drop(gate);
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn registration_relation_failure_rolls_back_attachment_upsert() {
    let pool = registration_pool().await;
    sqlx::query("DROP TABLE message_attachments")
        .execute(&pool)
        .await
        .expect("remove relation table for failure fixture");
    let hash = "f".repeat(64);
    let gate = attachment_gc_gate().read().await;
    let result = commit_registered_attachment_unlocked(
        &pool,
        &hash,
        "application/octet-stream",
        7,
        "/attachments/f.bin",
        11,
        &gate,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(registered_attachment_count(&pool, &hash).await, 0);
    drop(gate);
}

#[tokio::test]
async fn existing_cas_reuse_preserves_persisted_metadata_and_rejects_size_mismatch() {
    let fixture = registration_app().await;
    let pool = registration_pool().await;
    let content = b"existing CAS bytes";
    let hash = calculate_sha256(content);
    let path = fixture.root.join(format!("{hash}.bin"));
    fs::write(&path, content).expect("write existing CAS");
    insert_existing_cas(&pool, &hash, &path, content.len() as u64).await;
    let gate = attachment_gc_gate().read().await;
    let data = register_attachment_internal_unlocked(
        fixture.app.handle(),
        &pool,
        AttachmentRegistrationInput::new(
            hash.clone(),
            "incoming-name.pdf".to_string(),
            "text/plain".to_string(),
            content.len() as u64,
            path.to_string_lossy().into_owned(),
        ),
        &gate,
    )
    .await
    .expect("reuse existing CAS");
    let persisted = attachment_metadata(&pool, &hash).await;
    assert_eq!(data.mime_type, "application/pdf");
    assert_eq!(data.internal_path, path.to_string_lossy());
    assert_eq!(data.created_at, 123);
    assert_eq!(persisted.0, "application/pdf");
    assert_eq!(persisted.1, path.to_string_lossy());
    assert_eq!(persisted.2, content.len() as i64);
    assert_eq!(persisted.3, 123);

    let mismatch = register_attachment_internal_unlocked(
        fixture.app.handle(),
        &pool,
        AttachmentRegistrationInput::new(
            hash.clone(),
            "wrong-size.pdf".to_string(),
            "text/plain".to_string(),
            content.len() as u64 + 1,
            path.to_string_lossy().into_owned(),
        ),
        &gate,
    )
    .await;
    assert!(mismatch.is_err());
    assert_eq!(attachment_metadata(&pool, &hash).await, persisted);
    drop(gate);
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn unlocked_registration_does_not_nest_read_lock_behind_waiting_writer() {
    let fixture = registration_app().await;
    let pool = registration_pool().await;
    let content = b"unlocked gate registration";
    let hash = calculate_sha256(content);
    let path = fixture.root.join(format!("{hash}.bin"));
    fs::write(&path, content).expect("write unlocked CAS");
    let read_gate = attachment_gc_gate().read().await;
    let (writer, started_rx, queued_rx, acquired_rx) = spawn_queued_writer();
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .expect("writer should start")
        .expect("writer start signal");
    tokio::time::timeout(Duration::from_secs(2), queued_rx)
        .await
        .expect("writer should enter fair queue")
        .expect("writer queue signal");
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        register_attachment_internal_unlocked(
            fixture.app.handle(),
            &pool,
            AttachmentRegistrationInput::new(
                hash,
                "unlocked.bin".to_string(),
                "application/octet-stream".to_string(),
                content.len() as u64,
                path.to_string_lossy().into_owned(),
            ),
            &read_gate,
        ),
    )
    .await
    .expect("unlocked registration should not deadlock")
    .expect("unlocked registration");
    assert_eq!(result.size, content.len() as u64);
    drop(read_gate);
    tokio::time::timeout(Duration::from_secs(2), acquired_rx)
        .await
        .expect("writer should acquire after read registration")
        .expect("writer acquire signal");
    writer.await.expect("join writer");
    let _ = fs::remove_file(path);
}
