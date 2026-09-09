use super::ndjson_codec::{http_status_error, read_response_limited};
use super::{AvatarPullTarget, PullExecutor};
use crate::vcp_modules::db_write_queue::{DbWriteQueue, DbWriteTask};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Runtime};

const MAX_AVATAR_RETRIES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AvatarMime {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl AvatarMime {
    fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

#[derive(Debug)]
struct AvatarPullError {
    message: String,
    retryable: bool,
}

impl AvatarPullError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

impl std::fmt::Display for AvatarPullError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

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
        Self::pull_avatar_with_expected_hash(
            _app,
            client,
            http_url,
            sync_token,
            AvatarPullTarget {
                owner_type,
                owner_id,
                expected_hash: None,
            },
            write_queue,
        )
        .await
    }

    /// Pull and validate one avatar, optionally against the manifest hash.
    /// The compatibility facade above keeps legacy callers source-compatible;
    /// manifest-aware callers should provide `expected_hash`.
    pub async fn pull_avatar_with_expected_hash<R: Runtime>(
        _app: &AppHandle<R>,
        client: &reqwest::Client,
        http_url: &str,
        sync_token: &str,
        target: AvatarPullTarget<'_>,
        write_queue: &DbWriteQueue,
    ) -> Result<(), String> {
        if !crate::vcp_modules::sync_types::is_valid_avatar_owner(
            target.owner_type,
            target.owner_id,
        ) {
            return Err(format!(
                "Invalid avatar owner {}/{}",
                target.owner_type, target.owner_id
            ));
        }
        if let Some(expected_hash) = target.expected_hash {
            validate_expected_avatar_hash(expected_hash)?;
        }
        let url = format!("{http_url}/api/mobile-sync/avatars/pull");
        let mut attempt = 0usize;
        loop {
            match request_avatar_once(client, &url, sync_token, target, write_queue).await {
                Ok(()) => return Ok(()),
                Err(error) if error.retryable && attempt < MAX_AVATAR_RETRIES => {
                    attempt += 1;
                    let delay_ms = 200u64.saturating_mul(1u64 << (attempt - 1));
                    log::warn!(
                        "[PullExecutor] Avatar {}/{} failed; retry {attempt}/{MAX_AVATAR_RETRIES} in {delay_ms}ms: {error}",
                        target.owner_type, target.owner_id
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }
                Err(error) => {
                    return Err(format!(
                        "Pull avatar failed after {} attempt(s): {}",
                        attempt + 1,
                        error.message
                    ));
                }
            }
        }
    }
}

async fn request_avatar_once(
    client: &reqwest::Client,
    url: &str,
    sync_token: &str,
    target: AvatarPullTarget<'_>,
    write_queue: &DbWriteQueue,
) -> Result<(), AvatarPullError> {
    let response = client
        .get(url)
        .query(&[
            ("ownerType", target.owner_type),
            ("ownerId", target.owner_id),
        ])
        .header("Authorization", format!("Bearer {sync_token}"))
        .send()
        .await
        .map_err(|error| AvatarPullError::retryable(format!("avatar request failed: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        return read_avatar_http_error(response, status).await;
    }

    let content_type = avatar_content_type(&response)?;
    let bytes = read_avatar_body(response).await?;
    let hash = validate_avatar_bytes(&bytes, &content_type).map_err(AvatarPullError::permanent)?;
    if let Some(expected_hash) = target.expected_hash {
        if !hash.eq_ignore_ascii_case(expected_hash) {
            return Err(AvatarPullError::permanent(format!(
                "Avatar sha256 {hash} does not match expected manifest hash"
            )));
        }
    }
    log::debug!(
        "[PullExecutor] Avatar {}/{} validated ({content_type}, sha256={hash})",
        target.owner_type,
        target.owner_id
    );
    write_queue
        .submit(DbWriteTask::Avatar {
            owner_type: target.owner_type.to_string(),
            owner_id: target.owner_id.to_string(),
            bytes,
        })
        .await
        .map_err(AvatarPullError::permanent)
}

async fn read_avatar_http_error(
    response: reqwest::Response,
    status: reqwest::StatusCode,
) -> Result<(), AvatarPullError> {
    let retryable = is_retryable_avatar_status(status);
    let (_, bytes) = read_response_limited(
        response,
        super::MAX_ERROR_RESPONSE_BYTES,
        "Pull avatar error",
    )
    .await
    .map_err(|error| {
        if retryable {
            AvatarPullError::retryable(error)
        } else {
            AvatarPullError::permanent(error)
        }
    })?;
    let message = http_status_error("Pull avatar", status, &bytes);
    Err(if retryable {
        AvatarPullError::retryable(message)
    } else {
        AvatarPullError::permanent(message)
    })
}

fn is_retryable_avatar_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn avatar_content_type(response: &reqwest::Response) -> Result<String, AvatarPullError> {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AvatarPullError::permanent("Avatar response requires Content-Type"))
}

async fn read_avatar_body(response: reqwest::Response) -> Result<Vec<u8>, AvatarPullError> {
    let (_, bytes) =
        read_response_limited(response, super::MAX_AVATAR_RESPONSE_BYTES, "Pull avatar")
            .await
            .map_err(classify_avatar_body_error)?;
    Ok(bytes)
}

fn classify_avatar_body_error(error: String) -> AvatarPullError {
    if error.contains("response exceeds") {
        AvatarPullError::permanent(error)
    } else {
        AvatarPullError::retryable(error)
    }
}

fn validate_expected_avatar_hash(hash: &str) -> Result<(), String> {
    if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Avatar manifest hash must be a 64-character SHA-256 hex string".to_string())
    }
}

/// Validate size, allowlisted MIME, magic bytes and the computed SHA-256.
/// The returned hash is the only stable identity available for the binary.
pub(crate) fn validate_avatar_bytes(bytes: &[u8], content_type: &str) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("Avatar response body is empty".to_string());
    }
    if bytes.len() > super::MAX_AVATAR_RESPONSE_BYTES {
        return Err(format!(
            "Avatar response exceeds {} bytes",
            super::MAX_AVATAR_RESPONSE_BYTES
        ));
    }
    let declared = parse_avatar_mime(content_type)?;
    let detected = detect_avatar_mime(bytes)
        .ok_or_else(|| "Avatar response has unsupported or invalid magic bytes".to_string())?;
    if declared != detected {
        return Err(format!(
            "Avatar Content-Type {} conflicts with detected {}",
            declared.content_type(),
            detected.content_type()
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn parse_avatar_mime(value: &str) -> Result<AvatarMime, String> {
    match value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Ok(AvatarMime::Png),
        "image/jpeg" | "image/jpg" => Ok(AvatarMime::Jpeg),
        "image/gif" => Ok(AvatarMime::Gif),
        "image/webp" => Ok(AvatarMime::Webp),
        other => Err(format!("Avatar Content-Type {other} is not allowlisted")),
    }
}

fn detect_avatar_mime(bytes: &[u8]) -> Option<AvatarMime> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(AvatarMime::Png);
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some(AvatarMime::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(AvatarMime::Gif);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(AvatarMime::Webp);
    }
    None
}
