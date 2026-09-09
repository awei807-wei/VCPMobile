use chrono::TimeZone;
use serde_json::{json, Value};

fn extract_text_for_hash(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .filter(|part| part["type"].as_str() == Some("text"))
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    content["text"].as_str().unwrap_or_default().to_string()
}

fn get_or_calculate_message_hash(content: &Value) -> String {
    use crate::vcp_modules::infra::utils::calculate_sha256;

    format!(
        "sha256:{}",
        calculate_sha256(extract_text_for_hash(content).as_bytes())
    )
}

/// 抽取历史消息时间戳绑定，同时移除不应进入网络正文的内部元数据。
pub(super) fn extract_timestamp_bindings(messages: &mut [Value]) -> Vec<Value> {
    let mut bindings = Vec::new();
    for (index, message) in messages.iter_mut().enumerate() {
        let Some(meta) = message
            .as_object_mut()
            .and_then(|object| object.remove("__vcpchatTimestampMeta"))
        else {
            continue;
        };
        let (Some(message_id), Some(role), Some(timestamp)) = (
            meta.get("messageId").and_then(Value::as_str),
            meta.get("role").and_then(Value::as_str),
            meta.get("timestamp").and_then(Value::as_u64),
        ) else {
            continue;
        };
        let timestamp_iso = chrono::Utc
            .timestamp_millis_opt(timestamp as i64)
            .single()
            .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .unwrap_or_default();
        bindings.push(json!({
            "messageId": message_id,
            "role": role,
            "timestamp": timestamp,
            "timestampIso": timestamp_iso,
            "source": "client_history",
            "sentMessageHash": get_or_calculate_message_hash(
                message.get("content").unwrap_or(&Value::Null)
            ),
            "sentMessageIndex": index
        }));
    }
    bindings
}
