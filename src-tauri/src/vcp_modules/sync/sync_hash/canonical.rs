use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Serialize JSON using the desktop's UTF-8 byte key ordering.
///
/// Object keys are sorted by UTF-8 bytes before being emitted and are escaped
/// with the JSON serializer. This is deliberately independent of
/// `serde_json::Map`'s feature-dependent map ordering.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(values) => {
            let mut result = String::from("[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    result.push(',');
                }
                result.push_str(&canonical_json(value));
            }
            result.push(']');
            result
        }
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));

            let mut result = String::from("{");
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    result.push(',');
                }
                let encoded_key =
                    serde_json::to_string(*key).unwrap_or_else(|_| "\"\"".to_string());
                result.push_str(&encoded_key);
                result.push(':');
                result.push_str(&canonical_json(
                    values.get(*key).expect("key collected from object"),
                ));
            }
            result.push('}');
            result
        }
    }
}

/// Compute SHA-256 over the desktop-compatible canonical JSON representation.
pub fn compute_canonical_hash<T: Serialize>(data: &T) -> String {
    let value = serde_json::to_value(data).unwrap_or(Value::Null);
    let canonical = canonical_json(&value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(crate) fn object(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

pub(crate) fn string(value: impl Into<String>) -> Value {
    Value::String(value.into())
}
