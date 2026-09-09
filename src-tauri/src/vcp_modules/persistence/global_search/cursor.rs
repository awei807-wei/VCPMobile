use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use super::query::SortMode;

const CURSOR_VERSION: u8 = 1;
const MAX_CURSOR_BYTES: usize = 4096;
static CURSOR_SECRET: OnceLock<[u8; 32]> = OnceLock::new();

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct CursorPayload {
    pub(crate) version: u8,
    pub(crate) fingerprint: String,
    pub(crate) sort: String,
    pub(crate) timestamp: i64,
    pub(crate) rank_bits: Option<u64>,
    pub(crate) owner_type: String,
    pub(crate) owner_id: String,
    pub(crate) topic_id: String,
    pub(crate) msg_id: String,
    #[serde(default)]
    pub(crate) checksum: String,
}

pub(crate) fn encode_cursor(mut payload: CursorPayload) -> Result<String, String> {
    payload.version = CURSOR_VERSION;
    payload.checksum.clear();
    payload.checksum = checksum(&payload)?;
    let bytes = serde_json::to_vec(&payload).map_err(|_| "搜索游标编码失败".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn decode_cursor(encoded: &str) -> Result<CursorPayload, String> {
    if encoded.len() > MAX_CURSOR_BYTES {
        return Err("搜索游标过长".to_string());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "搜索游标格式错误".to_string())?;
    if bytes.len() > MAX_CURSOR_BYTES {
        return Err("搜索游标过长".to_string());
    }
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| "搜索游标格式错误".to_string())?;
    if payload.version != CURSOR_VERSION || payload.checksum != checksum(&payload)? {
        return Err("搜索游标完整性校验失败".to_string());
    }
    Ok(payload)
}

pub(crate) fn sort_name(sort: SortMode) -> &'static str {
    match sort {
        SortMode::Time => "time",
        SortMode::Rank => "rank",
    }
}

fn checksum(payload: &CursorPayload) -> Result<String, String> {
    let mut unsigned = payload.clone();
    unsigned.checksum.clear();
    let bytes = serde_json::to_vec(&unsigned).map_err(|_| "搜索游标编码失败".to_string())?;
    let secret = CURSOR_SECRET.get_or_init(|| {
        let mut value = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut value);
        value
    });
    let mut digest = Sha256::new();
    digest.update(b"vcp-mobile-global-search-cursor-v1\0");
    digest.update(secret);
    digest.update(bytes);
    Ok(hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::decode_cursor;

    #[test]
    fn 游标长度和格式均有边界保护() {
        assert_eq!(
            decode_cursor(&"A".repeat(4097)).unwrap_err(),
            "搜索游标过长"
        );
        assert_eq!(
            decode_cursor("not-a-cursor").unwrap_err(),
            "搜索游标格式错误"
        );
    }
}
