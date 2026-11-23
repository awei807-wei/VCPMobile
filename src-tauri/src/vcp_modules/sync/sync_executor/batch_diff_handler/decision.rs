use super::error::Phase3ProtocolError;
use super::protocol::{MAX_PHASE3_MESSAGES_PER_TOPIC, MAX_SAFE_JSON_INTEGER};
use crate::vcp_modules::sync_executor::{MessagePushSelection, MessagePushTarget};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, MessageDiffDecision};
use crate::vcp_modules::topic_types::TopicKey;
use crate::vcp_modules::wire_protocol::WireProtocolVersion;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TopicDecision {
    pub(crate) pull_all: bool,
    pub(crate) pull_message_ids: Vec<String>,
    pub(crate) push_target: Option<MessagePushTarget>,
    pub(crate) delete_messages: Vec<MessageDeleteDecision>,
}

#[cfg(test)]
pub(crate) fn parse_topic_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    parse_topic_decision_for_wire(topic, decision, WireProtocolVersion::V1_6)
}

pub(crate) fn parse_topic_decision_for_wire(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
    wire_version: WireProtocolVersion,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if !decision.ok {
        return parse_rejected_decision(topic, decision);
    }
    match wire_version {
        WireProtocolVersion::V1_6 => parse_accepted_decision_v1_6(topic, decision),
        WireProtocolVersion::V1_5 => parse_accepted_decision_v1_5(topic, decision),
    }
}

fn parse_rejected_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if decision.pull_all.is_some()
        || decision.pull_message_ids.is_some()
        || decision.push_topic.is_some()
        || decision.push_message_ids.is_some()
        || decision.push_deleted_messages.is_some()
        || decision.delete_messages.is_some()
    {
        return Err(invalid(topic, "rejection must not contain decision fields"));
    }
    let error = decision
        .error
        .clone()
        .ok_or_else(|| invalid(topic, "rejection requires an error object"))?;
    Err(Phase3ProtocolError::from_wire(error, &topic.topic_id).unwrap_or_else(|error| error))
}

fn parse_accepted_decision_v1_6(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if decision.error.is_some() {
        return Err(invalid(topic, "success must not contain an error"));
    }
    if decision.push_topic.is_some() {
        return Err(invalid(
            topic,
            "Wire 1.6 decision must not contain legacy pushTopic",
        ));
    }
    let pull_all = decision
        .pull_all
        .ok_or_else(|| invalid(topic, "success requires boolean pullAll"))?;
    let pull_message_ids = decision
        .pull_message_ids
        .clone()
        .ok_or_else(|| invalid(topic, "success requires pullMessageIds"))?;
    validate_id_list(topic, "pullMessageIds", &pull_message_ids)?;
    let push_message_ids = decision
        .push_message_ids
        .clone()
        .ok_or_else(|| invalid(topic, "success requires pushMessageIds"))?;
    validate_id_list(topic, "pushMessageIds", &push_message_ids)?;
    let push_deleted_messages = decision
        .push_deleted_messages
        .clone()
        .ok_or_else(|| invalid(topic, "success requires pushDeletedMessages"))?;
    validate_tombstone_list(topic, "pushDeletedMessages", &push_deleted_messages)?;
    let delete_messages = decision
        .delete_messages
        .clone()
        .ok_or_else(|| invalid(topic, "success requires deleteMessages"))?;
    validate_tombstone_list(topic, "deleteMessages", &delete_messages)?;

    if pull_all
        && (!pull_message_ids.is_empty()
            || !push_message_ids.is_empty()
            || !push_deleted_messages.is_empty()
            || !delete_messages.is_empty())
    {
        return Err(invalid(
            topic,
            "pullAll=true requires empty message action arrays",
        ));
    }

    validate_disjoint_sets(
        topic,
        &pull_message_ids,
        &push_message_ids,
        &push_deleted_messages,
        &delete_messages,
    )?;

    let push_target =
        (!push_message_ids.is_empty() || !push_deleted_messages.is_empty()).then(|| {
            MessagePushTarget::Selected(MessagePushSelection {
                topic: topic.clone(),
                message_ids: push_message_ids,
                deleted_messages: push_deleted_messages,
            })
        });

    Ok(TopicDecision {
        pull_all,
        pull_message_ids,
        push_target,
        delete_messages,
    })
}

fn parse_accepted_decision_v1_5(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if decision.error.is_some() {
        return Err(invalid(topic, "success must not contain an error"));
    }
    if decision.pull_all.is_some()
        || decision.push_message_ids.is_some()
        || decision.push_deleted_messages.is_some()
    {
        return Err(invalid(
            topic,
            "Wire 1.5 decision must not contain Wire 1.6 fields",
        ));
    }
    let pull_message_ids = decision
        .pull_message_ids
        .clone()
        .ok_or_else(|| invalid(topic, "success requires pullMessageIds"))?;
    validate_id_list(topic, "pullMessageIds", &pull_message_ids)?;
    let push_topic = decision
        .push_topic
        .ok_or_else(|| invalid(topic, "success requires boolean pushTopic"))?;
    let delete_messages = decision
        .delete_messages
        .clone()
        .ok_or_else(|| invalid(topic, "success requires deleteMessages"))?;
    validate_tombstone_list(topic, "deleteMessages", &delete_messages)?;
    validate_disjoint_sets(topic, &pull_message_ids, &[], &[], &delete_messages)?;
    Ok(TopicDecision {
        pull_all: false,
        pull_message_ids,
        push_target: push_topic.then(|| MessagePushTarget::WholeTopic(topic.clone())),
        delete_messages,
    })
}

fn validate_id_list(
    topic: &TopicKey,
    field: &str,
    ids: &[String],
) -> Result<(), Phase3ProtocolError> {
    if ids.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(budget(
            topic,
            &format!("{field} exceeds the per-topic budget"),
        ));
    }
    let mut seen = HashSet::new();
    if ids.iter().any(|id| id.is_empty() || !seen.insert(id)) {
        return Err(invalid(
            topic,
            &format!("{field} contains an empty or duplicate id"),
        ));
    }
    Ok(())
}

fn validate_tombstone_list(
    topic: &TopicKey,
    field: &str,
    decisions: &[MessageDeleteDecision],
) -> Result<(), Phase3ProtocolError> {
    if decisions.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(budget(
            topic,
            &format!("{field} exceeds the per-topic budget"),
        ));
    }
    let mut seen = HashSet::new();
    for item in decisions {
        if item.msg_id.is_empty() || !seen.insert(&item.msg_id) {
            return Err(invalid(
                topic,
                &format!("{field} contains an empty or duplicate msgId"),
            ));
        }
        if !(0..=MAX_SAFE_JSON_INTEGER).contains(&item.deleted_at) {
            return Err(invalid(
                topic,
                &format!("{field} requires a safe deletedAt"),
            ));
        }
    }
    Ok(())
}

fn validate_disjoint_sets(
    topic: &TopicKey,
    pull_ids: &[String],
    push_ids: &[String],
    push_deleted: &[MessageDeleteDecision],
    delete_messages: &[MessageDeleteDecision],
) -> Result<(), Phase3ProtocolError> {
    let mut all = HashSet::new();
    for id in pull_ids.iter().chain(push_ids.iter()) {
        if !all.insert(id.as_str()) {
            return Err(invalid(
                topic,
                "message id appears in multiple Phase 3 action sets",
            ));
        }
    }
    for item in push_deleted.iter().chain(delete_messages.iter()) {
        if !all.insert(item.msg_id.as_str()) {
            return Err(invalid(
                topic,
                "message id appears in multiple Phase 3 action sets",
            ));
        }
    }
    Ok(())
}

fn invalid(topic: &TopicKey, detail: &str) -> Phase3ProtocolError {
    Phase3ProtocolError::for_topic(
        "PHASE3_DECISION_INVALID",
        format!("Phase 3 decision for {} {detail}", topic.topic_id),
        &topic.topic_id,
    )
}

fn budget(topic: &TopicKey, detail: &str) -> Phase3ProtocolError {
    Phase3ProtocolError::for_topic(
        "PHASE3_DECISION_BUDGET_EXCEEDED",
        format!("Phase 3 decision for {} {detail}", topic.topic_id),
        &topic.topic_id,
    )
}
