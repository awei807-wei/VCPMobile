use super::registry::error_definition;
use super::types::{
    WireSyncError, MAX_ERROR_MESSAGE_CHARS, MAX_FAILED_TOPIC_IDS, MAX_TOPIC_ID_CHARS,
};
use serde_json::Value;
use std::collections::HashSet;

const NON_WIRE_ERROR_CODE_PREFIXES: &[&str] = &["ERR_", "SQLITE_"];

pub(crate) fn is_valid_wire_code(code: &str) -> bool {
    if code.is_empty() || code.len() > 64 {
        return false;
    }

    let mut bytes = code.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_uppercase()) {
        return false;
    }
    if !bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_') {
        return false;
    }

    !is_platform_error_code(code)
        && !code.starts_with("EAI_")
        && !NON_WIRE_ERROR_CODE_PREFIXES
            .iter()
            .any(|prefix| code.starts_with(prefix))
}

fn is_platform_error_code(code: &str) -> bool {
    matches!(
        code,
        "E2BIG"
            | "EACCES"
            | "EADDRINUSE"
            | "EADDRNOTAVAIL"
            | "EAGAIN"
            | "EBADF"
            | "EBUSY"
            | "ECONNABORTED"
            | "ECONNREFUSED"
            | "ECONNRESET"
            | "EEXIST"
            | "EFAULT"
            | "EHOSTUNREACH"
            | "EINTR"
            | "EINVAL"
            | "EIO"
            | "EISDIR"
            | "ELOOP"
            | "EMFILE"
            | "EMSGSIZE"
            | "ENAMETOOLONG"
            | "ENETDOWN"
            | "ENETUNREACH"
            | "ENFILE"
            | "ENOBUFS"
            | "ENODEV"
            | "ENOENT"
            | "ENOMEM"
            | "ENOSPC"
            | "ENOTDIR"
            | "ENOTEMPTY"
            | "ENOTFOUND"
            | "ENOTSUP"
            | "EPERM"
            | "EPIPE"
            | "EROFS"
            | "ETIMEDOUT"
    )
}

pub(crate) fn sanitize_topic_ids<I>(topic_ids: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    topic_ids
        .into_iter()
        .filter(|id| {
            !id.is_empty() && id.chars().count() <= MAX_TOPIC_ID_CHARS && seen.insert(id.clone())
        })
        .take(MAX_FAILED_TOPIC_IDS)
        .collect()
}

pub(crate) fn validate_wire_error(error: WireSyncError) -> Result<WireSyncError, String> {
    if !is_valid_wire_code(&error.code) {
        return Err("error.code is invalid".to_owned());
    }
    if error.message.trim().is_empty() || error.message.chars().count() > MAX_ERROR_MESSAGE_CHARS {
        return Err("error.message is invalid".to_owned());
    }
    if error.failed_topic_ids.len() > MAX_FAILED_TOPIC_IDS
        || error
            .failed_topic_ids
            .iter()
            .any(|id| id.is_empty() || id.chars().count() > MAX_TOPIC_ID_CHARS)
        || error.failed_topic_ids.iter().collect::<HashSet<_>>().len()
            != error.failed_topic_ids.len()
    {
        return Err("error.failedTopicIds is invalid".to_owned());
    }
    if let Some(registered) = error_definition(&error.code) {
        if error.kind != registered.category || error.retry != registered.retry {
            return Err("error.kind or error.retry conflicts with its registered code".to_owned());
        }
    }
    Ok(error)
}

pub fn parse_wire_sync_error(value: &Value) -> Result<WireSyncError, String> {
    let error = serde_json::from_value::<WireSyncError>(value.clone())
        .map_err(|parse_error| format!("invalid Wire 1.2 error object: {parse_error}"))?;
    validate_wire_error(error)
}
