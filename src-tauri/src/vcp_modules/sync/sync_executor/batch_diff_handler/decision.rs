use super::codec::MAX_PHASE3_MESSAGES;
use super::error::Phase3ProtocolError;
use crate::vcp_modules::sync_error::parse_wire_sync_error;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TopicDecision {
    pub(crate) to_pull: Vec<String>,
    pub(crate) to_push: bool,
}

pub(crate) fn parse_topic_decision(
    topic_id: &str,
    value: &Value,
) -> Result<TopicDecision, Phase3ProtocolError> {
    let object = value.as_object().ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} must be an object"),
            topic_id,
        )
    })?;
    let ok = object.get("ok").and_then(Value::as_bool).ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 decision for {topic_id} requires boolean ok"),
            topic_id,
        )
    })?;
    if !ok {
        return parse_rejection(topic_id, object);
    }
    parse_success(topic_id, object)
}

fn parse_rejection(
    topic_id: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if object.len() != 2
        || !object
            .keys()
            .all(|key| matches!(key.as_str(), "ok" | "error"))
    {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 rejection for {topic_id} must contain exactly ok/error"),
            topic_id,
        ));
    }
    let error = object.get("error").ok_or_else(|| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 rejection for {topic_id} requires error object"),
            topic_id,
        )
    })?;
    let wire = parse_wire_sync_error(error).map_err(|parse_error| {
        Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!("Phase 3 rejection for {topic_id} has invalid error: {parse_error}"),
            topic_id,
        )
    })?;
    Err(Phase3ProtocolError::from_wire(wire, topic_id).unwrap_or_else(|error| error))
}

fn parse_success(
    topic_id: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if object.len() != 3
        || !object
            .keys()
            .all(|key| matches!(key.as_str(), "ok" | "toPull" | "toPush"))
    {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_INVALID",
            format!(
                "Successful Phase 3 decision for {topic_id} must contain exactly ok/toPull/toPush"
            ),
            topic_id,
        ));
    }
    let to_pull_values = object
        .get("toPull")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 decision for {topic_id} requires string[] toPull"),
                topic_id,
            )
        })?;
    if to_pull_values.len() > MAX_PHASE3_MESSAGES {
        return Err(Phase3ProtocolError::for_topic(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!("Phase 3 toPull for {topic_id} exceeds {MAX_PHASE3_MESSAGES} message budget"),
            topic_id,
        ));
    }
    let to_pull = parse_message_ids(topic_id, to_pull_values)?;
    let to_push = object
        .get("toPush")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 decision for {topic_id} requires boolean toPush"),
                topic_id,
            )
        })?;
    Ok(TopicDecision { to_pull, to_push })
}

fn parse_message_ids(topic_id: &str, values: &[Value]) -> Result<Vec<String>, Phase3ProtocolError> {
    let mut seen = HashSet::new();
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        let message_id = value.as_str().filter(|id| !id.is_empty()).ok_or_else(|| {
            Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toPull for {topic_id} contains a non-string or empty id"),
                topic_id,
            )
        })?;
        if !seen.insert(message_id) {
            return Err(Phase3ProtocolError::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 toPull for {topic_id} contains duplicate id {message_id}"),
                topic_id,
            ));
        }
        ids.push(message_id.to_string());
    }
    Ok(ids)
}
