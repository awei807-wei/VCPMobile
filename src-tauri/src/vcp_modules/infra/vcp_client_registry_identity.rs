use crate::vcp_modules::chat::topic_types::{MessageKey, OwnerKey, TopicKey};
use serde_json::Value;

/// 根据请求上下文构建完整消息身份。
pub fn message_key_from_context(
    context: Option<&Value>,
    message_id: &str,
) -> Result<MessageKey, String> {
    let context = context
        .ok_or_else(|| "VCP 请求需要 ownerType/ownerId 和 topicId 以构建复合身份".to_string())?;
    let object = context
        .as_object()
        .ok_or_else(|| "VCP 请求 context 必须是包含复合所有者身份的对象".to_string())?;

    let owner_type = required_string_field(object, "ownerType")?;
    if owner_type != "agent" && owner_type != "group" {
        return Err("ownerType 必须是 agent 或 group".to_string());
    }
    if let Some(alias) = optional_string_field(object, "owner_type")? {
        if alias != owner_type {
            return Err("ownerType 与 owner_type 冲突".to_string());
        }
        return Err("必须使用显式 ownerType 字段".to_string());
    }
    validate_context_message_id(object, "messageId", message_id)?;
    validate_context_message_id(object, "message_id", message_id)?;
    validate_context_message_id(object, "requestId", message_id)?;
    validate_context_message_id(object, "request_id", message_id)?;

    let generic_owner_id = optional_string_field(object, "ownerId")?;
    let group_id = optional_string_field(object, "groupId")?;
    let agent_id = optional_string_field(object, "agentId")?;
    let owner_id = match owner_type {
        "group" => select_consistent_owner_id(generic_owner_id, group_id, "ownerId", "groupId")?,
        "agent" => {
            if group_id.is_some() {
                return Err("agent owner 不得携带 groupId".to_string());
            }
            select_consistent_owner_id(generic_owner_id, agent_id, "ownerId", "agentId")?
        }
        _ => unreachable!(),
    };
    let topic_id = required_string_field(object, "topicId")?;

    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    if !key.is_valid() {
        return Err(format!(
            "VCP 请求的复合身份无效：{}/{}/{}/{}",
            key.topic.owner_type, key.topic.owner_id, key.topic.topic_id, key.msg_id
        ));
    }
    Ok(key)
}

fn optional_string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(format!("{name} 必须是字符串")),
    }
}

fn required_string_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    optional_string_field(object, name)?.ok_or_else(|| format!("缺少必需字段 {name}"))
}

fn select_consistent_owner_id<'a>(
    generic: Option<&'a str>,
    specific: Option<&'a str>,
    generic_name: &str,
    specific_name: &str,
) -> Result<&'a str, String> {
    match (generic, specific) {
        (Some(generic), Some(specific)) if generic != specific => {
            Err(format!("{generic_name} 与 {specific_name} 冲突"))
        }
        (Some(owner_id), _) | (_, Some(owner_id)) => Ok(owner_id),
        (None, None) => Err(format!("缺少必需字段 {specific_name} 或 {generic_name}")),
    }
}

fn validate_context_message_id(
    object: &serde_json::Map<String, Value>,
    field: &str,
    payload_message_id: &str,
) -> Result<(), String> {
    if let Some(context_message_id) = optional_string_field(object, field)? {
        if context_message_id != payload_message_id {
            return Err(format!("上下文 {field} 与 payload.messageId 不一致"));
        }
    }
    Ok(())
}

/// 根据显式参数构建完整消息身份。
pub fn message_key_from_parts(
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    message_id: &str,
) -> Result<MessageKey, String> {
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err(format!(
            "复合身份无效：{owner_type}/{owner_id}/{topic_id}/{message_id}"
        ))
    }
}

/// 解析可选的旧版消息身份参数。
pub fn optional_message_key(
    message_id: &str,
    owner_id: Option<String>,
    owner_type: Option<String>,
    topic_id: Option<String>,
) -> Result<Option<MessageKey>, String> {
    match (owner_id, owner_type, topic_id) {
        (None, None, None) => Ok(None),
        (Some(owner_id), Some(owner_type), Some(topic_id)) => {
            message_key_from_parts(&owner_id, &owner_type, &topic_id, message_id).map(Some)
        }
        _ => Err("ownerId、ownerType 和 topicId 必须同时提供以构建复合身份".to_string()),
    }
}

/// 为需要生成上下文的调用方构建稳定的所有者身份。
pub fn owner_key_from_parts(owner_id: &str, owner_type: &str) -> Result<OwnerKey, String> {
    let key = OwnerKey::new(owner_type, owner_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err(format!("所有者身份无效：{owner_type}/{owner_id}"))
    }
}
