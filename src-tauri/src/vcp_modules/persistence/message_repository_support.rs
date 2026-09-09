use crate::vcp_modules::message_repository::RENDERER_SCHEMA_VERSION;
use crate::vcp_modules::persistence::message_content_storage;
use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
use sqlx::Row;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

pub(crate) const MAX_SAFE_JSON_INTEGER: u64 = (1_u64 << 53) - 1;
pub(crate) type CachedMessageSource = (MessageKey, String, String);
pub(crate) type RenderCacheWrite = (MessageKey, String, Vec<u8>);

pub(crate) fn open_maintenance_rusqlite(
    db_path: &std::path::Path,
) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|error| error.to_string())?;
    conn.execute("PRAGMA journal_mode = WAL", []).ok();
    conn.execute("PRAGMA synchronous = NORMAL", []).ok();
    conn.execute("PRAGMA busy_timeout = 30000", []).ok();
    Ok(conn)
}

/// Resolve a monotonic message activity clock while keeping replays idempotent.
pub(crate) fn resolve_message_updated_at(
    explicit_updated_at: Option<u64>,
    message_timestamp: u64,
    content_hash: &str,
    existing: Option<(&str, i64)>,
    now: i64,
) -> Result<i64, String> {
    if let Some((previous_hash, previous_updated_at)) = existing {
        let resolved = if previous_hash == content_hash {
            previous_updated_at
        } else {
            now.max(previous_updated_at.saturating_add(1))
        };
        if resolved < 0 || resolved as u64 > MAX_SAFE_JSON_INTEGER {
            return Err("message updatedAt exceeds the safe integer range".to_string());
        }
        return Ok(resolved);
    }
    let updated_at = explicit_updated_at.unwrap_or(message_timestamp);
    if updated_at > MAX_SAFE_JSON_INTEGER {
        return Err("message timestamp exceeds the safe integer range".to_string());
    }
    i64::try_from(updated_at).map_err(|_| "message updatedAt is too large".to_string())
}

pub(crate) async fn stream_cached_message_contents(
    pool: &sqlx::SqlitePool,
    tx: mpsc::Sender<CachedMessageSource>,
) -> Result<(), String> {
    let mut last_rowid = 0_i64;
    const FETCH_SIZE: i64 = 500;
    loop {
        let rows = sqlx::query(
            "SELECT m.rowid, m.owner_type, m.owner_id, m.topic_id, m.msg_id,
                    m.content, m.content_hash
             FROM messages m
             INNER JOIN render_cache r
               ON m.owner_type = r.owner_type AND m.owner_id = r.owner_id
              AND m.topic_id = r.topic_id AND m.msg_id = r.msg_id
             WHERE m.rowid > ?
             ORDER BY m.rowid
             LIMIT ?",
        )
        .bind(last_rowid)
        .bind(FETCH_SIZE)
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取预渲染缓存来源失败: {error}"))?;
        if rows.is_empty() {
            break;
        }
        if let Some(last) = rows.last() {
            last_rowid = last.get::<i64, _>(0);
        }
        for row in rows {
            let owner_type: String = row.get("owner_type");
            let owner_id: String = row.get("owner_id");
            let topic_id: String = row.get("topic_id");
            let msg_id: String = row.get("msg_id");
            let content = message_content_storage::decode_message_content(&row, "content")
                .map_err(|error| {
                    format!(
                        "读取消息 {owner_type}/{owner_id}/{topic_id}/{msg_id} 正文失败: {error}"
                    )
                })?;
            let content_hash: String = row.get("content_hash");
            let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), msg_id);
            if tx.send((key, content, content_hash)).await.is_err() {
                return Err(
                    "render cache compiler channel closed before source scan completed".to_string(),
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn update_render_cache_if_current(
    conn: &rusqlite::Connection,
    key: &MessageKey,
    content_hash: &str,
    bytes: &[u8],
    now: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE render_cache SET
            render_content = ?1,
            content_hash = ?2,
            renderer_schema_version = ?3,
            updated_at = ?4
         WHERE owner_type = ?5 AND owner_id = ?6 AND topic_id = ?7 AND msg_id = ?8
           AND EXISTS (
             SELECT 1 FROM messages
             WHERE owner_type = ?5 AND owner_id = ?6 AND topic_id = ?7 AND msg_id = ?8
               AND content_hash = ?2 AND deleted_at IS NULL
           )",
        rusqlite::params![
            bytes,
            content_hash,
            RENDERER_SCHEMA_VERSION,
            now,
            &key.topic.owner_type,
            &key.topic.owner_id,
            &key.topic.topic_id,
            &key.msg_id,
        ],
    )
}

pub(crate) fn run_render_cache_update_writer(
    db_path: &std::path::Path,
    mut rx: mpsc::Receiver<Vec<RenderCacheWrite>>,
    progress_event: &str,
    app_handle: AppHandle,
    total: usize,
) -> tokio::task::JoinHandle<Result<(), String>> {
    let progress_event = progress_event.to_string();
    let db_path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = open_maintenance_rusqlite(&db_path)?;
        let mut processed = 0;
        let mut last_emit_time = std::time::Instant::now();
        let emit_interval = std::time::Duration::from_millis(32);
        while let Some(batch) = rx.blocking_recv() {
            let tx = conn.transaction().map_err(|error| error.to_string())?;
            let now = chrono::Utc::now().timestamp_millis();
            for (key, content_hash, bytes) in batch {
                update_render_cache_if_current(&tx, &key, &content_hash, &bytes, now)
                    .map_err(|error| error.to_string())?;
                processed += 1;
            }
            tx.commit().map_err(|error| error.to_string())?;
            if last_emit_time.elapsed() >= emit_interval || processed == total {
                let _ = app_handle.emit(
                    &progress_event,
                    super::message_repository_render::RebuildProgress {
                        current: processed,
                        total,
                    },
                );
                last_emit_time = std::time::Instant::now();
            }
        }
        Ok(())
    })
}
