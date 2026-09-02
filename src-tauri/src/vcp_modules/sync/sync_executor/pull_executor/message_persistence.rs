use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use crate::vcp_modules::message_repository::{ContentCompressor, MessageRenderCompiler};
use crate::vcp_modules::sync_hash::HashAggregator;
use sqlx::Row;
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Manager, Runtime};

type PreparedMessages = (
    Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    Vec<String>,
    Vec<Vec<u8>>,
    Vec<Vec<u8>>,
);

/// Resolve local attachment CAS paths, compile optional renders, compress text, and enqueue rows.
pub(crate) async fn process_topic_messages<R: Runtime>(
    app: &AppHandle<R>,
    topic_id: &str,
    mut messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    write_queue: &DbWriteQueue,
    prerender_enabled: bool,
) -> Result<(usize, usize), String> {
    let started = std::time::Instant::now();
    let attachment_started = std::time::Instant::now();
    let db = app.state::<DbState>();
    let path_map = resolve_attachment_paths(&db.pool, &messages).await?;
    fill_attachment_paths(&mut messages, &path_map);
    let attachment_time = attachment_started.elapsed();
    let parsed_count = messages.len();
    if parsed_count > 0 {
        let block_started = std::time::Instant::now();
        let prepared =
            compile_messages_async(messages, topic_id.to_string(), prerender_enabled).await?;
        let block_time = block_started.elapsed();
        let submit_started = std::time::Instant::now();
        submit_message_chunks(write_queue, topic_id, prepared).await?;
        let submit_time = submit_started.elapsed();
        log::debug!(
            "[PullExecutor] [ProfileDetail] topic={} msgs={} | sql_att={:?} spawn_blocking={:?} submit_queue={:?} | total_proc={:?}",
            topic_id,
            parsed_count,
            attachment_time,
            block_time,
            submit_time,
            started.elapsed()
        );
    }
    Ok((parsed_count, 0))
}

async fn resolve_attachment_paths(
    pool: &sqlx::SqlitePool,
    messages: &[crate::vcp_modules::chat_manager::ChatMessage],
) -> Result<HashMap<String, String>, String> {
    let hashes = collect_attachment_hashes(messages);
    let mut path_map = HashMap::new();
    for chunk in hashes.chunks(500) {
        if chunk.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let query_text =
            format!("SELECT hash, internal_path FROM attachments WHERE hash IN ({placeholders})");
        let mut query = sqlx::query(&query_text);
        for hash in chunk {
            query = query.bind(hash);
        }
        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|error| format!("Failed to resolve local attachment CAS paths: {error}"))?;
        decode_attachment_rows(rows, &mut path_map).await?;
    }
    Ok(path_map)
}

fn collect_attachment_hashes(
    messages: &[crate::vcp_modules::chat_manager::ChatMessage],
) -> Vec<String> {
    let mut hashes = HashSet::new();
    for message in messages {
        if let Some(attachments) = &message.attachments {
            for attachment in attachments {
                if let Some(hash) = &attachment.hash {
                    if !hash.is_empty() {
                        hashes.insert(hash.clone());
                    }
                }
            }
        }
    }
    hashes.into_iter().collect()
}

async fn decode_attachment_rows(
    rows: Vec<sqlx::sqlite::SqliteRow>,
    path_map: &mut HashMap<String, String>,
) -> Result<(), String> {
    for row in rows {
        let hash = row
            .try_get::<String, _>("hash")
            .map_err(|error| format!("Failed to decode attachment hash: {error}"))?;
        let path = row
            .try_get::<String, _>("internal_path")
            .map_err(|error| format!("Failed to decode attachment path: {error}"))?;
        let clean_path = path.trim_start_matches("file://");
        if clean_path.is_empty() {
            continue;
        }
        match tokio::fs::metadata(clean_path).await {
            Ok(metadata) if metadata.is_file() => {
                path_map.insert(hash, clean_path.to_string());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect local attachment {hash}: {error}"
                ));
            }
        }
    }
    Ok(())
}

fn fill_attachment_paths(
    messages: &mut [crate::vcp_modules::chat_manager::ChatMessage],
    path_map: &HashMap<String, String>,
) {
    for message in messages {
        let Some(attachments) = &mut message.attachments else {
            continue;
        };
        for attachment in attachments {
            let Some(hash) = &attachment.hash else {
                continue;
            };
            if hash.is_empty() {
                continue;
            }
            if let Some(path) = path_map.get(hash) {
                attachment.internal_path = path.clone();
                attachment.src = format!("file://{path}");
                attachment.status = Some("ready".to_string());
            } else {
                attachment.internal_path.clear();
                attachment.src.clear();
                attachment.status = Some("desktop_only".to_string());
            }
        }
    }
}

async fn compile_messages_async(
    messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    topic_id: String,
    prerender_enabled: bool,
) -> Result<PreparedMessages, String> {
    tokio::task::spawn_blocking(move || compile_messages(messages, &topic_id, prerender_enabled))
        .await
        .map_err(|error| format!("Spawn blocking failed: {error}"))?
}

fn compile_messages(
    messages: Vec<crate::vcp_modules::chat_manager::ChatMessage>,
    topic_id: &str,
    prerender_enabled: bool,
) -> Result<PreparedMessages, String> {
    let mut content_hashes = Vec::with_capacity(messages.len());
    let mut render_bytes = Vec::with_capacity(messages.len());
    let mut compressed_contents = Vec::with_capacity(messages.len());
    for message in &messages {
        let attachment_hashes = message
            .attachments
            .as_ref()
            .map(|attachments| {
                attachments
                    .iter()
                    .filter_map(|attachment| attachment.hash.clone())
                    .filter(|hash| !hash.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        content_hashes.push(HashAggregator::compute_message_fingerprint(
            &message.content,
            &attachment_hashes,
        ));
        render_bytes.push(compile_render(message, topic_id, prerender_enabled));
        compressed_contents.push(ContentCompressor::compress(&message.content)?);
    }
    Ok((messages, content_hashes, render_bytes, compressed_contents))
}

fn compile_render(
    message: &crate::vcp_modules::chat_manager::ChatMessage,
    topic_id: &str,
    prerender_enabled: bool,
) -> Vec<u8> {
    if !prerender_enabled {
        return Vec::new();
    }
    let message_id = message.id.clone();
    let content = &message.content;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let blocks = MessageRenderCompiler::compile(content);
        MessageRenderCompiler::serialize(&blocks).unwrap_or_default()
    })) {
        Ok(render) => render,
        Err(_) => {
            log::warn!(
                "[PullExecutor] Compile panicked for msg {} (topic {})",
                message_id,
                topic_id
            );
            Vec::new()
        }
    }
}

async fn submit_message_chunks(
    write_queue: &DbWriteQueue,
    topic_id: &str,
    prepared: PreparedMessages,
) -> Result<(), String> {
    const WRITE_CHUNK_MESSAGES: usize = 250;
    let (messages, hashes, renders, compressed) = prepared;
    let mut messages = messages.into_iter();
    let mut hashes = hashes.into_iter();
    let mut renders = renders.into_iter();
    let mut compressed = compressed.into_iter();
    loop {
        let chunk: Vec<_> = messages.by_ref().take(WRITE_CHUNK_MESSAGES).collect();
        if chunk.is_empty() {
            return Ok(());
        }
        let chunk_len = chunk.len();
        write_queue
            .submit(DbWriteTask::TopicMessages {
                topic_id: topic_id.to_string(),
                messages: chunk,
                compressed_contents: compressed.by_ref().take(chunk_len).collect(),
                render_bytes: renders.by_ref().take(chunk_len).collect(),
                content_hashes: hashes.by_ref().take(chunk_len).collect(),
                skip_bubble: true,
            })
            .await?;
    }
}
