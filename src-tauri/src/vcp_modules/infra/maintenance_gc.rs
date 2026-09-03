use std::collections::HashSet;
use std::path::Path;

use tauri::{AppHandle, State};

use crate::vcp_modules::db_manager::DbState;
use crate::vcp_modules::file_manager::{
    delete_attachment_physical, get_attachments_root_dir, get_multimodal_cache_dir,
    get_thumbnails_root_dir,
};
use crate::vcp_modules::infra::utils::{is_valid_cas_hash, YieldCounter};

pub(super) async fn calculate_dir_size(path: &Path) -> u64 {
    let mut total_size = 0;
    let mut stack = vec![path.to_path_buf()];
    let mut yield_ctrl = YieldCounter::new(200);
    while let Some(current_path) = stack.pop() {
        if current_path.is_file() {
            yield_ctrl.tick().await;
            if let Ok(meta) = tokio::fs::metadata(&current_path).await {
                total_size += meta.len();
            }
        } else if current_path.is_dir() {
            if let Ok(mut entries) = tokio::fs::read_dir(&current_path).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    stack.push(entry.path());
                }
            }
        }
    }
    total_size
}

#[tauri::command]
pub async fn cleanup_orphaned_attachments(
    app_handle: AppHandle,
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    let attachments_dir = get_attachments_root_dir(&app_handle)?;
    scrub_deleted_attachment_links(&db_state).await;
    let used_hashes = collect_used_hashes(&db_state).await?;
    let (deleted_count, freed_size) =
        remove_unreferenced_index_entries(&app_handle, &db_state, &used_hashes).await?;
    let current_hashes = load_current_hashes(&db_state).await;
    let (ghost_deleted_count, ghost_freed_size) =
        sweep_ghost_files(&app_handle, &attachments_dir, &current_hashes).await;
    let total_deleted = deleted_count + ghost_deleted_count;
    let total_freed_mb = ((freed_size + ghost_freed_size) as f64) / 1024.0 / 1024.0;
    Ok(format!(
        "清理完成：共删除 {} 个孤立文件 (常规: {} 个，幽灵: {} 个)，释放空间: {:.2} MB",
        total_deleted, deleted_count, ghost_deleted_count, total_freed_mb
    ))
}

async fn scrub_deleted_attachment_links(db_state: &State<'_, DbState>) {
    let _ = sqlx::query(
        "UPDATE message_attachments
         SET display_name = '[附件已删除]', src = NULL, status = 'removed'
         WHERE deleted_at IS NOT NULL
            OR EXISTS (
                SELECT 1 FROM messages m
                WHERE m.owner_type = message_attachments.owner_type
                  AND m.owner_id = message_attachments.owner_id
                  AND m.topic_id = message_attachments.topic_id
                  AND m.msg_id = message_attachments.msg_id
                  AND m.deleted_at IS NOT NULL
            )
            OR EXISTS (
                SELECT 1 FROM topics t
                WHERE t.owner_type = message_attachments.owner_type
                  AND t.owner_id = message_attachments.owner_id
                  AND t.topic_id = message_attachments.topic_id
                  AND (t.deleted_at IS NOT NULL
                    OR (t.owner_type = 'agent' AND EXISTS (
                        SELECT 1 FROM agents a WHERE a.agent_id = t.owner_id AND a.deleted_at IS NOT NULL
                    ))
                    OR (t.owner_type = 'group' AND EXISTS (
                        SELECT 1 FROM groups g WHERE g.group_id = t.owner_id AND g.deleted_at IS NOT NULL
                    )))
            )",
    )
    .execute(&db_state.pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM message_attachments AS ma WHERE EXISTS (
         SELECT 1 FROM messages m
         WHERE m.owner_type = ma.owner_type AND m.owner_id = ma.owner_id
           AND m.topic_id = ma.topic_id AND m.msg_id = ma.msg_id
           AND m.deleted_at IS NOT NULL)",
    )
    .execute(&db_state.pool)
    .await;
}

async fn collect_used_hashes(db_state: &State<'_, DbState>) -> Result<HashSet<String>, String> {
    let hashes: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT ma.hash FROM message_attachments ma
             INNER JOIN messages m ON ma.owner_type = m.owner_type AND ma.owner_id = m.owner_id
                AND ma.topic_id = m.topic_id AND ma.msg_id = m.msg_id
             INNER JOIN topics t ON m.owner_type = t.owner_type AND m.owner_id = t.owner_id
                AND m.topic_id = t.topic_id
             LEFT JOIN agents a ON t.owner_id = a.agent_id AND t.owner_type = 'agent'
             LEFT JOIN groups g ON t.owner_id = g.group_id AND t.owner_type = 'group'
             WHERE m.deleted_at IS NULL
               AND ma.deleted_at IS NULL
               AND t.deleted_at IS NULL
               AND (t.owner_type != 'agent' OR a.deleted_at IS NULL)
               AND (t.owner_type != 'group' OR g.deleted_at IS NULL)",
    )
    .fetch_all(&db_state.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(hashes.into_iter().map(|(hash,)| hash).collect())
}

async fn remove_unreferenced_index_entries(
    app_handle: &AppHandle,
    db_state: &State<'_, DbState>,
    used_hashes: &HashSet<String>,
) -> Result<(i32, u64), String> {
    let indexed: Vec<(String, String)> =
        sqlx::query_as("SELECT hash, internal_path FROM attachments")
            .fetch_all(&db_state.pool)
            .await
            .unwrap_or_default();
    let mut deleted_count = 0;
    let mut freed_size = 0;
    for (hash, local_path) in indexed {
        if used_hashes.contains(&hash) {
            continue;
        }
        let path = Path::new(&local_path);
        if path.exists() {
            if let Ok(meta) = tokio::fs::metadata(path).await {
                freed_size += meta.len();
            }
            let _ = delete_attachment_physical(app_handle, &hash, &local_path).await;
            deleted_count += 1;
        }
        let _ = sqlx::query("DELETE FROM attachments WHERE hash = ?")
            .bind(&hash)
            .execute(&db_state.pool)
            .await;
    }
    Ok((deleted_count, freed_size))
}

async fn load_current_hashes(db_state: &State<'_, DbState>) -> HashSet<String> {
    sqlx::query_as::<_, (String,)>("SELECT hash FROM attachments")
        .fetch_all(&db_state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(hash,)| hash)
        .collect()
}

async fn sweep_ghost_files(
    app_handle: &AppHandle,
    attachments_dir: &Path,
    current_hashes: &HashSet<String>,
) -> (i32, u64) {
    let mut deleted = sweep_hash_files(attachments_dir, current_hashes).await;
    if let Ok(thumbnails_dir) = get_thumbnails_root_dir(app_handle) {
        let result =
            sweep_named_files(&thumbnails_dir, current_hashes, GhostFileKind::Thumbnail).await;
        deleted = add_sweep_results(deleted, result);
    }
    if let Ok(cache_dir) = get_multimodal_cache_dir(app_handle) {
        let result = sweep_named_files(&cache_dir, current_hashes, GhostFileKind::Cache).await;
        deleted = add_sweep_results(deleted, result);
    }
    deleted
}

fn add_sweep_results(left: (i32, u64), right: (i32, u64)) -> (i32, u64) {
    (left.0 + right.0, left.1 + right.1)
}

async fn sweep_hash_files(dir: &Path, current_hashes: &HashSet<String>) -> (i32, u64) {
    sweep_named_files(dir, current_hashes, GhostFileKind::Attachment).await
}

#[derive(Clone, Copy)]
enum GhostFileKind {
    Attachment,
    Thumbnail,
    Cache,
}

async fn sweep_named_files(
    dir: &Path,
    current_hashes: &HashSet<String>,
    kind: GhostFileKind,
) -> (i32, u64) {
    let mut deleted = 0;
    let mut freed_size = 0;
    let mut file_yield = YieldCounter::new(200);
    if !dir.exists() {
        return (deleted, freed_size);
    }
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return (deleted, freed_size);
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        file_yield.tick().await;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let Some(hash) = ghost_file_hash(&file_name, kind) else {
            continue;
        };
        if current_hashes.contains(&hash) {
            continue;
        }
        if let Ok(meta) = tokio::fs::metadata(&path).await {
            freed_size += meta.len();
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            deleted += 1;
            log::info!("[Maintenance] GC swept ghost file: {}", file_name);
        }
    }
    (deleted, freed_size)
}

fn ghost_file_hash(file_name: &str, kind: GhostFileKind) -> Option<String> {
    match kind {
        GhostFileKind::Attachment => {
            let hash = file_name.split('.').next().unwrap_or(file_name);
            is_valid_cas_hash(hash).then(|| hash.to_string())
        }
        GhostFileKind::Thumbnail if file_name.ends_with("_thumb.webp") && file_name.len() == 75 => {
            Some(file_name[..64].to_string())
        }
        GhostFileKind::Cache if file_name.ends_with(".json") && file_name.len() == 69 => {
            Some(file_name[..64].to_string())
        }
        _ => None,
    }
}
