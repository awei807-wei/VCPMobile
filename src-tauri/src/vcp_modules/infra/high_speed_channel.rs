use crate::vcp_modules::db_manager::DbState;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(serde::Deserialize)]
pub struct UploadMetadata {
    pub name: String,
    pub mime: String,
    pub size: u64,
}

#[derive(serde::Serialize)]
pub struct UploadEndpoint {
    pub url: String,
    pub token: String,
}

/// 准备高速上传链路：启动临时本地服务器并返回端口。
#[tauri::command]
pub async fn prepare_vcp_upload<R: Runtime>(
    app_handle: AppHandle<R>,
    db_state: State<'_, DbState>,
    metadata: UploadMetadata,
) -> Result<UploadEndpoint, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let token = uuid::Uuid::new_v4().to_string();
    let endpoint = UploadEndpoint {
        url: format!("http://127.0.0.1:{port}"),
        token: token.clone(),
    };
    tauri::async_runtime::spawn(run_upload_worker(
        app_handle,
        db_state.pool.clone(),
        metadata,
        listener,
    ));
    Ok(endpoint)
}

async fn run_upload_worker<R: Runtime>(
    app_handle: AppHandle<R>,
    pool: sqlx::SqlitePool,
    metadata: UploadMetadata,
    listener: TcpListener,
) {
    let Some(temp_dir) = prepare_upload_temp_dir(&app_handle) else {
        return;
    };
    let timeout = std::time::Duration::from_secs(20);
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        let Ok(Ok((mut socket, _))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept()).await
        else {
            continue;
        };
        let temp_path = temp_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        match receive_upload(&mut socket, &temp_path, metadata.size).await {
            Ok(ReceivedUpload::Options) => {}
            Ok(ReceivedUpload::Incomplete) => {
                let _ = fs::remove_file(&temp_path);
                send_http_response(&mut socket, 400, b"Incomplete Data").await;
            }
            Ok(ReceivedUpload::Complete { hash, size }) => {
                let result = finalize_high_speed_upload(
                    &app_handle,
                    &pool,
                    &temp_path,
                    &metadata,
                    hash,
                    size,
                )
                .await;
                match result {
                    Ok(data) => {
                        let body = serde_json::to_vec(&data).unwrap_or_default();
                        send_http_response(&mut socket, 200, &body).await;
                    }
                    Err(error) => send_http_response(&mut socket, 500, error.as_bytes()).await,
                }
                break;
            }
            Err(error) => {
                let _ = fs::remove_file(&temp_path);
                send_http_response(&mut socket, 500, error.as_bytes()).await;
            }
        }
    }
}

fn prepare_upload_temp_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Option<PathBuf> {
    let path = app_handle.path().app_cache_dir().ok()?.join("uploads");
    fs::create_dir_all(&path).ok()?;
    Some(path)
}

enum ReceivedUpload {
    Options,
    Incomplete,
    Complete { hash: String, size: u64 },
}

struct UploadReceiveState {
    temp_path: PathBuf,
    header_data: Vec<u8>,
    body_started: bool,
    file: Option<tokio::fs::File>,
    hasher: Sha256,
    bytes_count: u64,
}

impl UploadReceiveState {
    fn new(temp_path: &Path) -> Self {
        Self {
            temp_path: temp_path.to_path_buf(),
            header_data: Vec::with_capacity(4096),
            body_started: false,
            file: None,
            hasher: Sha256::new(),
            bytes_count: 0,
        }
    }

    async fn append_body(&mut self, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        if self.file.is_none() {
            self.file = Some(
                tokio::fs::File::create(&self.temp_path)
                    .await
                    .map_err(|error| format!("创建上传临时文件失败: {error}"))?,
            );
        }
        self.file
            .as_mut()
            .expect("上传文件已创建")
            .write_all(data)
            .await
            .map_err(|error| format!("写入上传临时文件失败: {error}"))?;
        self.hasher.update(data);
        self.bytes_count += data.len() as u64;
        Ok(())
    }

    async fn finish(mut self, expected_size: u64) -> Result<ReceivedUpload, String> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .await
                .map_err(|error| format!("刷新上传临时文件失败: {error}"))?;
        }
        if self.bytes_count < expected_size {
            return Ok(ReceivedUpload::Incomplete);
        }
        Ok(ReceivedUpload::Complete {
            hash: hex::encode(self.hasher.finalize()),
            size: self.bytes_count,
        })
    }
}

enum HeaderChunk {
    Pending,
    Options,
    Body(Vec<u8>),
}

fn parse_header_chunk(
    header_data: &mut Vec<u8>,
    body_started: &mut bool,
    chunk: &[u8],
) -> HeaderChunk {
    if *body_started {
        return HeaderChunk::Body(chunk.to_vec());
    }
    header_data.extend_from_slice(chunk);
    let Some(position) = header_data
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
    else {
        return HeaderChunk::Pending;
    };
    *body_started = true;
    if header_data[..position].starts_with(b"OPTIONS") {
        return HeaderChunk::Options;
    }
    let body_start = position + 4;
    HeaderChunk::Body(header_data[body_start..].to_vec())
}

async fn receive_upload(
    socket: &mut TcpStream,
    temp_path: &Path,
    expected_size: u64,
) -> Result<ReceivedUpload, String> {
    let mut state = UploadReceiveState::new(temp_path);
    let mut buffer = [0_u8; 65_536];
    loop {
        let bytes = socket
            .read(&mut buffer)
            .await
            .map_err(|error| format!("读取上传请求失败: {error}"))?;
        if bytes == 0 {
            break;
        }
        match parse_header_chunk(
            &mut state.header_data,
            &mut state.body_started,
            &buffer[..bytes],
        ) {
            HeaderChunk::Pending => continue,
            HeaderChunk::Options => {
                send_options_response(socket).await;
                return Ok(ReceivedUpload::Options);
            }
            HeaderChunk::Body(data) => {
                state.append_body(&data).await?;
                if state.bytes_count >= expected_size {
                    break;
                }
            }
        }
    }
    state.finish(expected_size).await
}

async fn send_options_response(socket: &mut TcpStream) {
    let response = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Max-Age: 86400\r\nConnection: close\r\n\r\n";
    let _ = socket.write_all(response.as_bytes()).await;
}

async fn send_http_response(socket: &mut TcpStream, status: u16, body: &[u8]) {
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(body).await;
}

async fn finalize_high_speed_upload<R: Runtime>(
    app_handle: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    temp_path: &Path,
    metadata: &UploadMetadata,
    hash: String,
    size: u64,
) -> Result<crate::vcp_modules::file_manager::AttachmentData, String> {
    let gate = crate::vcp_modules::file_manager::attachment_gc_gate()
        .read()
        .await;
    let destination = publish_high_speed_file(app_handle, temp_path, metadata, &hash, size).await?;
    let result = crate::vcp_modules::file_manager::register_attachment_internal_unlocked(
        app_handle,
        pool,
        crate::vcp_modules::file_manager::AttachmentRegistrationInput::new(
            hash,
            metadata.name.clone(),
            metadata.mime.clone(),
            size,
            destination.path.to_string_lossy().into_owned(),
        ),
        &gate,
    )
    .await;
    match result {
        Ok(data) => Ok(data),
        Err(error) => {
            if destination.published {
                rollback_published_file(&destination.path).await;
            }
            Err(error)
        }
    }
}

struct PublishedDestination {
    path: PathBuf,
    published: bool,
}

async fn publish_high_speed_file<R: Runtime>(
    app_handle: &AppHandle<R>,
    temp_path: &Path,
    metadata: &UploadMetadata,
    hash: &str,
    size: u64,
) -> Result<PublishedDestination, String> {
    let root = crate::vcp_modules::file_manager::get_attachments_root_dir(app_handle)?;
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|error| format!("创建附件目录失败: {error}"))?;
    let extension = crate::vcp_modules::file_manager::safe_storage_extension(&metadata.name);
    let filename = extension
        .map(|value| format!("{hash}.{value}"))
        .unwrap_or_else(|| hash.to_string());
    let destination = root.join(filename);
    match tokio::fs::symlink_metadata(&destination).await {
        Ok(metadata) if metadata.file_type().is_file() => {
            crate::vcp_modules::file_manager::check_existing_cas_size_async(&destination, size)
                .await?;
            let _ = tokio::fs::remove_file(temp_path).await;
            Ok(PublishedDestination {
                path: destination,
                published: false,
            })
        }
        Ok(_) => Err("高速上传目标不是 regular file".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            crate::vcp_modules::file_manager::safe_rename(temp_path, &destination)
                .map_err(|error| format!("发布高速上传 CAS 文件失败: {error}"))?;
            Ok(PublishedDestination {
                path: destination,
                published: true,
            })
        }
        Err(error) => Err(format!("检查高速上传 CAS 文件失败: {error}")),
    }
}

async fn rollback_published_file(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await {
        log::warn!("高速上传数据库注册失败，清理已发布文件失败: {error}");
    }
}

#[cfg(test)]
#[path = "high_speed_channel_tests.rs"]
mod tests;
