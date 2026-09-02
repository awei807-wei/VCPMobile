use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffAction {
    Pull,
    Push,
    Delete,
    PushDelete,
    Skip,
}

impl DiffAction {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "PULL" => Some(Self::Pull),
            "PUSH" => Some(Self::Push),
            "DELETE" => Some(Self::Delete),
            "PUSH_DELETE" => Some(Self::PushDelete),
            "SKIP" => Some(Self::Skip),
            _ => None,
        }
    }

    pub(crate) fn is_delete(self) -> bool {
        matches!(self, Self::Delete | Self::PushDelete)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerIdentity {
    pub(crate) owner_type: String,
    pub(crate) owner_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedDiffItem {
    pub(crate) id: String,
    pub(crate) action: DiffAction,
    pub(crate) deleted_at: Option<i64>,
    pub(crate) owner: Option<OwnerIdentity>,
    pub(crate) topic_id: Option<String>,
    pub(crate) mismatched_content: bool,
}

pub(crate) fn validate_diff_frame<'a>(
    payload: &'a Value,
    data_type: &SyncDataType,
) -> Result<&'a [Value], String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "SYNC_DIFF_RESULTS must be an object".to_string())?;
    let common = ["type", "data", "dataType"];
    let exact_central = object.len() == common.len();
    let exact_legacy = object.len() == common.len() + 1 && object.contains_key("phase");
    if (!exact_central && !exact_legacy) || common.iter().any(|key| !object.contains_key(*key)) {
        return Err("SYNC_DIFF_RESULTS must contain exactly type, data and dataType, with only the legacy phase field optional".to_string());
    }
    let expected_data_type = data_type.to_string();
    if object.get("type").and_then(Value::as_str) != Some("SYNC_DIFF_RESULTS")
        || object.get("dataType").and_then(Value::as_str) != Some(expected_data_type.as_str())
    {
        return Err("SYNC_DIFF_RESULTS type or dataType does not match dispatch".to_string());
    }
    object
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| "SYNC_DIFF_RESULTS.data must be an array".to_string())
}

pub(crate) fn parse_delete_timestamp(
    item: &Value,
    id: &str,
    action: &str,
) -> Result<Option<i64>, String> {
    if !matches!(action, "DELETE" | "PUSH_DELETE") {
        return item
            .get("deletedAt")
            .map(|value| parse_non_negative_timestamp(value, id))
            .transpose();
    }
    item.get("deletedAt")
        .map(|value| parse_non_negative_timestamp(value, id))
        .transpose()?
        .ok_or_else(|| {
            format!(
                "SYNC_DIFF_RESULTS item {id} delete action requires a non-negative integer deletedAt"
            )
        })
        .map(Some)
}

fn parse_non_negative_timestamp(value: &Value, id: &str) -> Result<i64, String> {
    value
        .as_i64()
        .filter(|timestamp| *timestamp >= 0)
        .ok_or_else(|| {
            format!("SYNC_DIFF_RESULTS item {id} deletedAt must be non-negative integer")
        })
}

/// 校验 SYNC_DIFF_RESULTS 条目并过滤契约豁免的 default 话题动作。
/// 返回（参与计数与派发的有效条目，被豁免的 default 话题动作条数）。
pub(crate) fn validate_and_filter_diff_items(
    items: &[Value],
    data_type: &SyncDataType,
) -> Result<(Vec<Value>, u32), String> {
    let mut seen_ids = HashSet::new();
    let mut filtered = Vec::with_capacity(items.len());
    let mut exempt_default_topics = 0u32;
    for item in items {
        let id = required_id(item)?;
        let is_default_topic = *data_type == SyncDataType::Topic && id == "default";
        validate_diff_item(item, data_type, id, !is_default_topic)?;
        if *data_type == SyncDataType::Topic && id == "default" {
            exempt_default_topics += 1;
            continue;
        }
        if !seen_ids.insert(id.to_owned()) {
            return Err(format!("SYNC_DIFF_RESULTS contains duplicate id {id}"));
        }
        if *data_type == SyncDataType::Topic {
            let object = item
                .as_object()
                .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} must be an object"))?;
            let owner = validate_optional_owner(object, id)?
                .ok_or_else(|| format!("SYNC_DIFF_RESULTS topic {id} requires owner identity"))?;
            let _validated_identity = (owner.owner_type, owner.owner_id);
        }
        filtered.push(item.clone());
    }
    Ok((filtered, exempt_default_topics))
}

fn required_id(item: &Value) -> Result<&str, String> {
    item.as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "SYNC_DIFF_RESULTS item requires a non-empty id".to_string())
}

fn validate_diff_item(
    item: &Value,
    data_type: &SyncDataType,
    id: &str,
    require_topic_owner: bool,
) -> Result<(), String> {
    let object = item
        .as_object()
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} must be an object"))?;
    let action = object
        .get("action")
        .and_then(Value::as_str)
        .and_then(DiffAction::parse)
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} has an invalid action"))?;
    validate_exact_item_keys(object, data_type, action, id)?;
    parse_delete_timestamp(item, id, action_name(action))?;
    validate_optional_boolean(object, "mismatchedContent", id)?;
    let owner = validate_optional_owner(object, id)?;
    if *data_type == SyncDataType::Topic && require_topic_owner && owner.is_none() {
        return Err(format!(
            "SYNC_DIFF_RESULTS topic {id} requires ownerType agent/group and non-empty ownerId"
        ));
    }
    if *data_type == SyncDataType::Message {
        let topic_id = object.get("topicId");
        if action.is_delete() && !non_empty_string(topic_id) {
            return Err(format!(
                "SYNC_DIFF_RESULTS message {id} delete requires non-empty topicId"
            ));
        }
        if topic_id.is_some() && !non_empty_string(topic_id) {
            return Err(format!("SYNC_DIFF_RESULTS message {id} topicId is invalid"));
        }
    }
    Ok(())
}

fn validate_exact_item_keys(
    object: &Map<String, Value>,
    data_type: &SyncDataType,
    action: DiffAction,
    id: &str,
) -> Result<(), String> {
    let mut allowed = HashSet::from(["id", "action"]);
    if *data_type == SyncDataType::Topic {
        allowed.extend(["ownerType", "ownerId"]);
    }
    if *data_type == SyncDataType::Message && object.contains_key("topicId") {
        allowed.insert("topicId");
    }
    if action.is_delete() {
        allowed.insert("deletedAt");
    }
    if matches!(
        data_type,
        SyncDataType::Agent | SyncDataType::Group | SyncDataType::Topic
    ) && object.contains_key("mismatchedContent")
    {
        allowed.insert("mismatchedContent");
    }
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err(format!(
            "SYNC_DIFF_RESULTS item {id} contains missing or unknown fields"
        ));
    }
    Ok(())
}

fn validate_optional_boolean(
    object: &Map<String, Value>,
    key: &str,
    id: &str,
) -> Result<(), String> {
    if object.get(key).is_some_and(|value| !value.is_boolean()) {
        return Err(format!("SYNC_DIFF_RESULTS item {id} {key} must be boolean"));
    }
    Ok(())
}

fn validate_optional_owner(
    object: &Map<String, Value>,
    id: &str,
) -> Result<Option<OwnerIdentity>, String> {
    let owner_type = object.get("ownerType");
    let owner_id = object.get("ownerId");
    if owner_type.is_none() && owner_id.is_none() {
        return Ok(None);
    }
    let owner_type = owner_type
        .and_then(Value::as_str)
        .filter(|value| matches!(*value, "agent" | "group"))
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} ownerType must be agent/group"))?;
    let owner_id = owner_id
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} ownerId must be non-empty string"))?;
    Ok(Some(OwnerIdentity {
        owner_type: owner_type.to_string(),
        owner_id: owner_id.to_string(),
    }))
}

fn non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn action_name(action: DiffAction) -> &'static str {
    match action {
        DiffAction::Pull => "PULL",
        DiffAction::Push => "PUSH",
        DiffAction::Delete => "DELETE",
        DiffAction::PushDelete => "PUSH_DELETE",
        DiffAction::Skip => "SKIP",
    }
}

pub(crate) fn parse_validated_item(
    item: Value,
    data_type: &SyncDataType,
) -> Result<ValidatedDiffItem, String> {
    let id = required_id(&item)?.to_string();
    validate_diff_item(&item, data_type, &id, true)?;
    let object = item
        .as_object()
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} must be an object"))?;
    let action_name = object
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} has an invalid action"))?;
    let action = DiffAction::parse(action_name)
        .ok_or_else(|| format!("SYNC_DIFF_RESULTS item {id} has an invalid action"))?;
    let deleted_at = parse_delete_timestamp(&item, &id, action_name)?;
    let owner = validate_optional_owner(object, &id)?;
    let topic_id = object
        .get("topicId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mismatched_content = object
        .get("mismatchedContent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ValidatedDiffItem {
        id,
        action,
        deleted_at,
        owner,
        topic_id,
        mismatched_content,
    })
}
