use super::message_repository_support::{
    run_render_cache_update_writer, stream_cached_message_contents, CachedMessageSource,
    RenderCacheWrite,
};
use crate::vcp_modules::content_parser::{parse_content, ContentBlock};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

pub const RENDERER_SCHEMA_VERSION: i64 = 1;

pub struct MessageRenderCompiler;

impl MessageRenderCompiler {
    pub fn compile(content: &str) -> Vec<ContentBlock> {
        parse_content(content)
    }

    pub fn serialize(blocks: &[ContentBlock]) -> Result<Vec<u8>, String> {
        let json_bytes = serde_json::to_vec(blocks)
            .map_err(|error| format!("json serialize failed: {error}"))?;
        zstd::bulk::compress(&json_bytes, 3)
            .map_err(|error| format!("zstd compress failed: {error}"))
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Vec<ContentBlock>, String> {
        let decompressed = zstd::bulk::decompress(bytes, 16 * 1024 * 1024)
            .map_err(|error| format!("zstd decompress failed: {error}"))?;
        serde_json::from_slice(&decompressed)
            .map_err(|error| format!("json deserialize failed: {error}"))
    }
}

pub struct ContentCompressor;

impl ContentCompressor {
    pub fn compress(text: &str) -> Result<Vec<u8>, String> {
        zstd::bulk::compress(text.as_bytes(), 3)
            .map_err(|error| format!("zstd compress content failed: {error}"))
    }

    pub fn decompress(bytes: &[u8]) -> Result<String, String> {
        let decompressed = zstd::bulk::decompress(bytes, 16 * 1024 * 1024)
            .map_err(|error| format!("zstd decompress content failed: {error}"))?;
        String::from_utf8(decompressed)
            .map_err(|error| format!("content decompression not valid utf-8: {error}"))
    }
}

#[tauri::command]
pub async fn process_message_content(
    _app_handle: AppHandle,
    content: String,
) -> Result<Vec<ContentBlock>, String> {
    Ok(MessageRenderCompiler::compile(&content))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildProgress {
    pub current: usize,
    pub total: usize,
}

#[tauri::command]
pub async fn rebuild_all_pre_renders(app_handle: AppHandle) -> Result<(), String> {
    let db_state = app_handle.state::<crate::vcp_modules::db_manager::DbState>();
    let pool = db_state.pool.clone();
    let db_path = db_state.path.clone();
    let total = count_render_cache(&pool).await?;
    if total == 0 {
        return Ok(());
    }
    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::start_stream_service_inner(
        &app_handle,
        "[预渲染重建] VCP Mobile",
    );
    let (tx_compiler, rx_compiler) = mpsc::channel::<CachedMessageSource>(1000);
    let (tx_writer, rx_writer) = mpsc::channel::<Vec<RenderCacheWrite>>(100);
    let total_count = total as usize;
    let writer_handle = run_render_cache_update_writer(
        &db_path,
        rx_writer,
        "render_rebuild_progress",
        app_handle.clone(),
        total_count,
    );
    let compiler_handles = spawn_compiler_workers(rx_compiler, &tx_writer);
    let reader_handle =
        tokio::spawn(async move { stream_cached_message_contents(&pool, tx_compiler).await });
    let reader_result = match reader_handle.await {
        Ok(result) => result,
        Err(error) => Err(format!("render cache reader task failed: {error}")),
    };
    let compiler_results = futures_util::future::join_all(compiler_handles).await;
    drop(tx_writer);
    let writer_result = match writer_handle.await {
        Ok(result) => result,
        Err(error) => Err(format!("render cache writer task failed: {error}")),
    };
    #[cfg(target_os = "android")]
    let _ = tauri_plugin_vcp_mobile::stream::stop_stream_service_inner(
        &app_handle,
        "[预渲染重建] VCP Mobile",
    );
    reader_result?;
    for result in compiler_results {
        result.map_err(|error| format!("render cache compiler task failed: {error}"))??;
    }
    writer_result?;
    let _ = app_handle.emit(
        "render_rebuild_progress",
        RebuildProgress {
            current: total_count,
            total: total_count,
        },
    );
    Ok(())
}

async fn count_render_cache(pool: &sqlx::SqlitePool) -> Result<i64, String> {
    sqlx::query_scalar("SELECT COUNT(*) FROM render_cache")
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())
}

fn spawn_compiler_workers(
    rx_compiler: mpsc::Receiver<CachedMessageSource>,
    tx_writer: &mpsc::Sender<Vec<RenderCacheWrite>>,
) -> Vec<tokio::task::JoinHandle<Result<(), String>>> {
    let concurrency = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .clamp(2, 12);
    let rx_compiler = std::sync::Arc::new(tokio::sync::Mutex::new(rx_compiler));
    (0..concurrency)
        .map(|_| {
            let rx_clone = rx_compiler.clone();
            let tx_writer_clone = tx_writer.clone();
            tokio::task::spawn_blocking(move || compile_cached_batches(rx_clone, tx_writer_clone))
        })
        .collect()
}

fn compile_cached_batches(
    rx: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<CachedMessageSource>>>,
    tx: mpsc::Sender<Vec<RenderCacheWrite>>,
) -> Result<(), String> {
    let mut batch = Vec::with_capacity(50);
    loop {
        let item = {
            let mut rx = rx.blocking_lock();
            rx.blocking_recv()
        };
        match item {
            Some((key, content, content_hash)) => {
                let bytes = compile_cached_content(&content)
                    .map_err(|error| format!("render cache compile failed: {error}"))?;
                batch.push((key, content_hash, bytes));
                if batch.len() >= 50 {
                    tx.blocking_send(std::mem::take(&mut batch)).map_err(|_| {
                        "render cache writer channel closed during rebuild".to_string()
                    })?;
                }
            }
            None => {
                if !batch.is_empty() {
                    tx.blocking_send(batch).map_err(|_| {
                        "render cache writer channel closed before final batch".to_string()
                    })?;
                }
                return Ok(());
            }
        }
    }
}

fn compile_cached_content(content: &str) -> Result<Vec<u8>, String> {
    let blocks = MessageRenderCompiler::compile(content);
    MessageRenderCompiler::serialize(&blocks)
}

#[cfg(test)]
mod tests {
    use super::compile_cached_batches;
    use crate::vcp_modules::topic_types::{MessageKey, TopicKey};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    #[test]
    fn compiler_reports_closed_writer_channel() {
        let (source_tx, source_rx) = mpsc::channel(1);
        let (writer_tx, writer_rx) = mpsc::channel(1);
        drop(writer_rx);
        source_tx
            .blocking_send((
                MessageKey::new(TopicKey::new("agent", "owner", "topic"), "message"),
                "body".to_string(),
                "hash".to_string(),
            ))
            .expect("queue source row");
        drop(source_tx);
        let error = compile_cached_batches(Arc::new(Mutex::new(source_rx)), writer_tx)
            .expect_err("closed writer must fail the rebuild");
        assert!(error.contains("writer channel closed"));
    }
}
