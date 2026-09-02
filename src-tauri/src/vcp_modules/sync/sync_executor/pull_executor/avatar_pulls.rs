use super::ndjson_codec::{http_status_error, read_response_limited};
use super::PullExecutor;
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use tauri::{AppHandle, Runtime};

const MAX_AVATAR_RESPONSE_BYTES: usize = 20 * 1024 * 1024;

impl PullExecutor {
    pub async fn pull_avatar<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        owner_type: &str,
        owner_id: &str,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        if !crate::vcp_modules::sync_types::is_valid_avatar_owner(owner_type, owner_id) {
            return Err(format!("Invalid avatar owner {owner_type}/{owner_id}"));
        }
        request_avatar_with_retries(
            client,
            http_url,
            sync_token,
            owner_type,
            owner_id,
            write_queue,
        )
        .await
    }
}

async fn request_avatar_with_retries(
    client: &reqwest::Client,
    http_url: &str,
    sync_token: &str,
    owner_type: &str,
    owner_id: &str,
    write_queue: &DbWriteQueue,
) -> Result<(), String> {
    let url = format!("{http_url}/api/mobile-sync/download-avatar?id={owner_id}&type={owner_type}");
    let mut retries = 0;
    let mut delay_ms = 200u64;
    loop {
        match request_avatar_once(client, &url, sync_token, owner_type, owner_id, write_queue).await
        {
            Ok(()) => return Ok(()),
            Err(error) if retries < 3 => {
                retries += 1;
                log::warn!(
                    "[PullExecutor] Avatar {owner_type} {owner_id} failed (retry {retries}/3): {error}. Waiting {delay_ms}ms"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                delay_ms *= 2;
            }
            Err(error) => return Err(format!("Pull avatar failed after 3 retries: {error}")),
        }
    }
}

async fn request_avatar_once(
    client: &reqwest::Client,
    url: &str,
    sync_token: &str,
    owner_type: &str,
    owner_id: &str,
    write_queue: &DbWriteQueue,
) -> Result<(), String> {
    let response = client
        .get(url)
        .header("x-sync-token", sync_token)
        .header("Authorization", format!("Bearer {sync_token}"))
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;
    let (status, bytes) =
        read_response_limited(response, MAX_AVATAR_RESPONSE_BYTES, "Pull avatar").await?;
    if !status.is_success() {
        return Err(http_status_error("Pull avatar", status, &bytes));
    }
    write_queue
        .submit(DbWriteTask::Avatar {
            owner_type: owner_type.to_string(),
            owner_id: owner_id.to_string(),
            bytes,
        })
        .await
}
