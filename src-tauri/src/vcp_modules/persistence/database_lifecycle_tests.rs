use super::*;
use rusqlite::Connection as RusqliteConnection;
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_database_path(name: &str) -> (PathBuf, PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("vcp_db_lifecycle_{name}_{nonce}"));
    fs::create_dir_all(&root).expect("test directory should be created");
    (root.clone(), root.join("vcp_avatar.db"))
}

fn archive_main_path(root: &std::path::Path) -> PathBuf {
    fs::read_dir(root)
        .expect("archive directory should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("vcp_avatar.db.corrupt."))
                && !name_has_sidecar_suffix(path)
        })
        .expect("archived main database should exist")
}

fn name_has_sidecar_suffix(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("-wal") || name.ends_with("-shm"))
}

fn archive_exists(root: &std::path::Path) -> bool {
    fs::read_dir(root)
        .expect("archive directory should be readable")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("vcp_avatar.db.corrupt."))
        })
}

#[test]
fn transient_sqlite_failures_are_not_recovery_candidates() {
    for code in [SQLITE_BUSY, SQLITE_LOCKED, SQLITE_IOERR] {
        assert!(!is_confirmed_corruption_code(Some(code)));
    }
    assert!(is_confirmed_corruption_code(Some(SQLITE_CORRUPT)));
    assert!(is_confirmed_corruption_code(Some(SQLITE_NOTADB)));
}

#[tokio::test]
async fn locked_database_is_not_archived_or_rebuilt() {
    let (root, db_path) = test_database_path("locked_database");
    let connection = RusqliteConnection::open(&db_path).expect("SQLite fixture should open");
    connection
        .execute_batch("CREATE TABLE locked_rows (value TEXT); BEGIN EXCLUSIVE;")
        .expect("exclusive lock should be acquired");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
        .busy_timeout(std::time::Duration::from_millis(1));

    let error = open_database_with_recovery(&options, &db_path)
        .await
        .expect_err("locked database must not enter recovery");
    assert!(error.contains("暂时不可用"));
    assert!(db_path.exists());
    assert!(!archive_exists(&root));

    drop(connection);
    fs::remove_dir_all(root).expect("test directory should be removed");
}

#[test]
fn corrupt_database_archive_moves_main_wal_and_shm_as_one_set() {
    let (root, db_path) = test_database_path("complete_set");
    fs::write(&db_path, b"main database bytes").expect("main fixture should be written");
    fs::write(sidecar_path(&db_path, "-wal"), b"wal bytes").expect("WAL fixture should be written");
    fs::write(sidecar_path(&db_path, "-shm"), b"SHM bytes").expect("SHM fixture should be written");

    archive_corrupt_db(&db_path).expect("complete database set should archive");
    let archived = archive_main_path(&root);

    assert!(!db_path.exists());
    assert!(!sidecar_path(&db_path, "-wal").exists());
    assert!(!sidecar_path(&db_path, "-shm").exists());
    assert_eq!(fs::read(&archived).unwrap(), b"main database bytes");
    assert_eq!(
        fs::read(sidecar_path(&archived, "-wal")).unwrap(),
        b"wal bytes"
    );
    assert_eq!(
        fs::read(sidecar_path(&archived, "-shm")).unwrap(),
        b"SHM bytes"
    );

    fs::remove_dir_all(root).expect("test directory should be removed");
}

#[test]
fn archive_failure_preserves_sidecars_and_leaves_original_path_empty() {
    let (root, db_path) = test_database_path("failed_archive");
    fs::write(sidecar_path(&db_path, "-wal"), b"wal bytes").expect("WAL fixture should be written");
    fs::write(sidecar_path(&db_path, "-shm"), b"SHM bytes").expect("SHM fixture should be written");

    let error = archive_corrupt_db(&db_path).expect_err("missing main database must fail closed");
    assert!(error.contains("主数据库文件不存在"));
    assert!(!db_path.exists());
    assert_eq!(
        fs::read(sidecar_path(&db_path, "-wal")).unwrap(),
        b"wal bytes"
    );
    assert_eq!(
        fs::read(sidecar_path(&db_path, "-shm")).unwrap(),
        b"SHM bytes"
    );

    fs::remove_dir_all(root).expect("test directory should be removed");
}

#[test]
fn archived_uncheckpointed_wal_can_be_reopened_with_committed_row() {
    let (root, db_path) = test_database_path("wal_row");
    let connection = RusqliteConnection::open(&db_path).expect("SQLite fixture should open");
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("WAL mode should be enabled");
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("WAL autocheckpoint should be disabled");
    connection
        .execute_batch(
            "CREATE TABLE recovered_rows (value TEXT NOT NULL);
             INSERT INTO recovered_rows(value) VALUES ('committed-in-wal');",
        )
        .expect("committed WAL fixture should be written");
    assert!(sidecar_path(&db_path, "-wal").exists());
    assert!(sidecar_path(&db_path, "-shm").exists());

    archive_corrupt_db(&db_path).expect("WAL database set should archive");
    let archived = archive_main_path(&root);
    let recovered = RusqliteConnection::open(&archived).expect("archived WAL set should open");
    let value: String = recovered
        .query_row("SELECT value FROM recovered_rows", [], |row| row.get(0))
        .expect("committed WAL row should be recovered");
    assert_eq!(value, "committed-in-wal");

    drop(recovered);
    drop(connection);
    fs::remove_dir_all(root).expect("test directory should be removed");
}
