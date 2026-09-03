use serde::de::Error as _;
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::io::{self, Write};

pub const SYNC_TOMBSTONE_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub const MAX_MANIFEST_ITEMS: usize = 10_000;
pub const MAX_TOPIC_DIFF_ITEMS: usize = 10_000;
pub const MAX_MESSAGE_DIFF_TOPICS: usize = 10_000;
pub const MAX_MESSAGE_DIFF_ITEMS: usize = 100_000;
pub const MAX_MESSAGES_PER_TOPIC: usize = 10_000;
pub(crate) const MAX_SAFE_TIMESTAMP: i64 = 9_007_199_254_740_991;

/// JavaScript's `Number.isSafeInteger` upper bound used by every numeric
/// value crossing the Wire 1.4 JSON boundary.  Keeping this predicate next
/// to the timestamp deserializers prevents DTOs from silently accepting a
/// value that would round differently in the desktop plugin.
pub(crate) fn is_safe_non_negative_u64(value: u64) -> bool {
    value <= MAX_SAFE_TIMESTAMP as u64
}

pub(crate) fn validate_safe_non_negative_u64(value: u64, field: &str) -> Result<u64, String> {
    if is_safe_non_negative_u64(value) {
        Ok(value)
    } else {
        Err(format!("{field} must be a non-negative safe integer"))
    }
}

pub(crate) fn deserialize_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if is_safe_non_negative_u64(value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(
            "value must be a non-negative safe integer",
        ))
    }
}

pub(crate) fn deserialize_optional_safe_u64<'de, D>(
    deserializer: D,
) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer)?
        .map(|value| {
            if is_safe_non_negative_u64(value) {
                Ok(value)
            } else {
                Err(serde::de::Error::custom(
                    "value must be a non-negative safe integer",
                ))
            }
        })
        .transpose()
}

pub(crate) fn serialize_safe_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if is_safe_non_negative_u64(*value) {
        serializer.serialize_u64(*value)
    } else {
        Err(serde::ser::Error::custom(
            "value must be a non-negative safe integer",
        ))
    }
}

pub(crate) fn serialize_optional_safe_u64<S>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(value) if is_safe_non_negative_u64(*value) => serializer.serialize_some(value),
        Some(_) => Err(serde::ser::Error::custom(
            "value must be a non-negative safe integer",
        )),
        None => serializer.serialize_none(),
    }
}

/// =================================================================
/// vcp_modules/sync_types.rs - 分布式 LWW+Hash 同步协议的核心数据结构
/// =================================================================
/// 计算 JSON 的确定性 SHA-256 Hash
pub fn compute_deterministic_hash<T: Serialize>(data: &T) -> String {
    let Ok(value) = serde_json::to_value(data) else {
        return String::new();
    };
    let mut hasher = Sha256::new();
    let result = {
        let mut writer = Sha256Writer(&mut hasher);
        write_stable_json(&value, &mut writer)
    };
    if result.is_err() {
        return String::new();
    }
    format!("{:x}", hasher.finalize())
}

/// 计算一组哈希的聚合哈希 (Merkle Root)
/// 规则：调用方先将实体身份绑定进叶子，再排序叶子哈希并计算总 Hash
pub fn compute_merkle_root(mut hashes: Vec<String>) -> String {
    if hashes.is_empty() {
        return "".to_string();
    }
    hashes.sort_unstable();
    let mut hasher = Sha256::new();
    for h in hashes {
        hasher.update(h.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub(crate) fn is_sha256(value: &str, allow_empty: bool) -> bool {
    (allow_empty && value.is_empty())
        || (value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

pub(crate) fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        return Err(D::Error::custom("identity string must not be empty"));
    }
    Ok(value)
}

pub(crate) fn deserialize_sha256<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if !is_sha256(&value, false) {
        return Err(D::Error::custom("value must be a lowercase SHA-256 hash"));
    }
    Ok(value)
}

pub(crate) fn deserialize_content_hash<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if !is_sha256(&value, true) {
        return Err(D::Error::custom(
            "value must be empty or a lowercase SHA-256 hash",
        ));
    }
    Ok(value)
}

pub(crate) fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    if !(0..=MAX_SAFE_TIMESTAMP).contains(&value) {
        return Err(D::Error::custom(
            "timestamp must be a non-negative safe integer",
        ));
    }
    Ok(value)
}

pub(crate) fn deserialize_optional_timestamp<'de, D>(
    deserializer: D,
) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer)?
        .map(|value| {
            if !(0..=MAX_SAFE_TIMESTAMP).contains(&value) {
                Err(D::Error::custom(
                    "timestamp must be a non-negative safe integer",
                ))
            } else {
                Ok(value)
            }
        })
        .transpose()
}

pub(crate) fn serialize_timestamp<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if (0..=MAX_SAFE_TIMESTAMP).contains(value) {
        serializer.serialize_i64(*value)
    } else {
        Err(serde::ser::Error::custom(
            "timestamp must be a non-negative safe integer",
        ))
    }
}

pub(crate) fn deserialize_bounded_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let values = Vec::<T>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("item count exceeds the Wire 1.4 budget"));
    }
    Ok(values)
}

struct Sha256Writer<'a>(&'a mut Sha256);

impl Write for Sha256Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_json_string<W: Write>(writer: &mut W, value: &str) -> io::Result<()> {
    serde_json::to_writer(writer, value).map_err(io::Error::other)
}

fn write_stable_json<W: Write>(value: &serde_json::Value, writer: &mut W) -> io::Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            writer.write_all(b"{")?;
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    writer.write_all(b",")?;
                }
                write_json_string(writer, key)?;
                writer.write_all(b":")?;
                let child = map.get(key).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "canonical JSON key disappeared")
                })?;
                write_stable_json(child, writer)?;
            }
            writer.write_all(b"}")
        }
        serde_json::Value::Array(arr) => {
            writer.write_all(b"[")?;
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    writer.write_all(b",")?;
                }
                write_stable_json(v, writer)?;
            }
            writer.write_all(b"]")
        }
        serde_json::Value::String(value) => write_json_string(writer, value),
        serde_json::Value::Number(value) => write!(writer, "{value}"),
        serde_json::Value::Bool(true) => writer.write_all(b"true"),
        serde_json::Value::Bool(false) => writer.write_all(b"false"),
        serde_json::Value::Null => writer.write_all(b"null"),
    }
}

pub fn stable_stringify(value: &serde_json::Value) -> String {
    let mut bytes = Vec::new();
    if write_stable_json(value, &mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8(bytes).unwrap_or_default()
}
