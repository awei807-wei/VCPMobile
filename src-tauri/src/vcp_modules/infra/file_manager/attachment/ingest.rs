use crate::vcp_modules::db_manager::DbState;
use serde::Deserialize;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, State};

use super::ingest_helpers::{
    cleanup_staged_source, prepare_staged_upload, process_staged_thumbnail, promote_staged_file,
    register_promoted_file, validate_memory_upload, write_memory_cas,
};
use super::registration::AttachmentData;
use super::validation::{resolve_attachment_cas_file, store_file_semaphore};
use crate::vcp_modules::infra::file_manager::attachment_gc_gate;
use crate::vcp_modules::infra::file_manager::get_refined_mime_type;

/// 存储文件到中心化附件目录 (内容寻址存储)
///
/// 【适用场景】非 Android 端的前端小文件上传 (<2MB) 及录音片段、二维码等内存数据。
/// Android 端不走此函数：Android 通过原生插件 `pick_file` 在 Native 层完成文件拷贝与
/// 哈希计算后，直接调用 `register_local_file` 进行零拷贝注册。
///
/// 后端兜底硬上限 100MB，防止前端异常或 IPC 绕过导致 OOM。
#[tauri::command]
pub async fn store_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    original_name: String,
    file_bytes: Vec<u8>,
    mime_type: String,
) -> Result<AttachmentData, String> {
    let _gate = attachment_gc_gate().read().await;
    validate_memory_upload(file_bytes.len())?;
    let _permit = store_file_semaphore()
        .acquire_owned()
        .await
        .map_err(|_| "文件存储执行器已关闭".to_string())?;
    let hash = crate::vcp_modules::infra::utils::calculate_sha256(&file_bytes);
    let attachments_dir = super::paths::get_attachments_root_dir(&app_handle)?;
    let internal_file_path =
        write_memory_cas(&attachments_dir, &hash, &original_name, &file_bytes)?;
    let internal_path_str = internal_file_path
        .to_str()
        .ok_or("无效的附件路径字符")?
        .to_string();
    let refined_mime = get_refined_mime_type(&internal_file_path, &original_name, &mime_type);
    super::registration::register_attachment_internal_unlocked(
        &app_handle,
        &db_state.pool,
        super::registration::AttachmentRegistrationInput::new(
            hash,
            original_name,
            refined_mime,
            file_bytes.len() as u64,
            internal_path_str,
        ),
        &_gate,
    )
    .await
}

/// 注册本地已有的文件（例如 Android Kotlin 端沙盒临时复制的大文件/硬解缩略图）
/// 彻底实现“前端零拷贝物理路径传输”
/// 注册本地已有文件到附件系统 (零拷贝移动)
///
/// 【适用场景】Android 端实际上传入口。原生插件 `pick_file` 已将文件从 Scoped Storage
/// 流式拷贝到 app_cache_dir 并完成 SHA-256 计算，本函数仅负责：
///   1. rename/move 到附件目录 (内容寻址去重)
///   2. 生成/复用缩略图
///   3. 提取文本内容 (如适用)
///   4. 写入 attachment_registry 数据库
/// 全程不加载文件内容到内存，实现真正的零拷贝。
#[tauri::command]
pub async fn register_local_file(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    request: Request<'_>,
) -> Result<AttachmentData, String> {
    let args = parse_register_request(&request)?;
    register_local_file_impl(&app_handle, &db_state, args).await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterLocalFileArgs {
    local_path: String,
    original_name: String,
    mime_type: Option<String>,
    thumbnail_path: Option<String>,
    stable_id: Option<String>,
    expected_hash: Option<String>,
}

fn parse_register_request(request: &Request<'_>) -> Result<RegisterLocalFileArgs, String> {
    match request.body() {
        InvokeBody::Json(value) => serde_json::from_value(value.clone())
            .map_err(|error| format!("本地附件注册参数无效: {error}")),
        InvokeBody::Raw(_) => Err("本地附件注册不接受二进制 IPC 请求".to_string()),
    }
}

async fn register_local_file_impl(
    app_handle: &AppHandle,
    db_state: &DbState,
    args: RegisterLocalFileArgs,
) -> Result<AttachmentData, String> {
    let _gate = attachment_gc_gate().read().await;
    let staging = prepare_staged_upload(
        app_handle,
        &args.local_path,
        args.thumbnail_path.as_deref(),
        args.expected_hash.as_deref(),
        args.stable_id.as_deref(),
    )
    .await?;
    let promoted = promote_staged_file(
        app_handle,
        &staging,
        &args.original_name,
        args.stable_id.as_deref(),
    )
    .await?;
    let mut attachment_data = match register_promoted_file(
        app_handle,
        db_state,
        &staging,
        &promoted,
        args.original_name,
        args.mime_type,
        &_gate,
    )
    .await
    {
        Ok(data) => data,
        Err(error) => {
            log::warn!("附件数据库注册失败；完整 CAS 文件保留，等待重试或维护 GC");
            return Err(error);
        }
    };
    cleanup_staged_source(&staging.source_path, promoted.placement).await;
    if let Some(source_thumbnail) = staging.thumbnail_path {
        match process_staged_thumbnail(
            app_handle,
            db_state,
            &source_thumbnail,
            &staging.hash,
            attachment_data.created_at,
            &_gate,
        )
        .await
        {
            Ok(path) => attachment_data.thumbnail_path = Some(path),
            Err(error) => {
                log::warn!("主附件已提交；外部派生缩略图处理失败，保留主附件成功结果: {error}")
            }
        }
    }
    Ok(attachment_data)
}

/// 移动端/桌面端原生文件选取与存储 (流式防 OOM 优化版)
#[tauri::command]
pub async fn get_attachment_real_path(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
    hash: String,
    _original_name: String,
) -> Result<String, String> {
    resolve_attachment_cas_file(&app_handle, &db_state.pool, &hash)
        .await
        .map(|record| record.path.to_string_lossy().into_owned())
}
