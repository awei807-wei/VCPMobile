use super::error::Phase3ProtocolError;
use super::protocol::{MAX_PHASE3_MESSAGES_PER_TOPIC, MAX_SAFE_JSON_INTEGER};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, MessageDiffDecision};
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TopicDecision {
    pub(crate) pull_message_ids: Vec<String>,
    pub(crate) push_topic: bool,
    pub(crate) delete_messages: Vec<MessageDeleteDecision>,
}

pub(crate) fn parse_topic_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if !decision.ok {
        return parse_rejected_decision(topic, decision);
    }
    parse_accepted_decision(topic, decision)
}

fn parse_rejected_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if decision.pull_message_ids.is_some()
        || decision.push_topic.is_some()
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

fn parse_accepted_decision(
    topic: &TopicKey,
    decision: &MessageDiffDecision,
) -> Result<TopicDecision, Phase3ProtocolError> {
    if decision.error.is_some() {
        return Err(invalid(topic, "success must not contain an error"));
    }
    let pull_message_ids = decision
        .pull_message_ids
        .clone()
        .ok_or_else(|| invalid(topic, "success requires pullMessageIds"))?;
    validate_pull_ids(topic, &pull_message_ids)?;
    let push_topic = decision
        .push_topic
        .ok_or_else(|| invalid(topic, "success requires boolean pushTopic"))?;
    let delete_messages = decision
        .delete_messages
        .clone()
        .ok_or_else(|| invalid(topic, "success requires deleteMessages"))?;
    validate_delete_decisions(topic, &pull_message_ids, &delete_messages)?;
    Ok(TopicDecision {
        pull_message_ids,
        push_topic,
        delete_messages,
    })
}

fn validate_pull_ids(topic: &TopicKey, ids: &[String]) -> Result<(), Phase3ProtocolError> {
    if ids.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(budget(topic, "pullMessageIds exceeds the per-topic budget"));
    }
    let mut seen = HashSet::new();
    if ids.iter().any(|id| id.is_empty() || !seen.insert(id)) {
        return Err(invalid(
            topic,
            "pullMessageIds contains an empty or duplicate id",
        ));
    }
    Ok(())
}

fn validate_delete_decisions(
    topic: &TopicKey,
    pull_ids: &[String],
    decisions: &[crate::vcp_modules::sync_types::MessageDeleteDecision],
) -> Result<(), Phase3ProtocolError> {
    if decisions.len() > MAX_PHASE3_MESSAGES_PER_TOPIC {
        return Err(budget(topic, "deleteMessages exceeds the per-topic budget"));
    }
    let pull_ids = pull_ids.iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    for item in decisions {
        if item.msg_id.is_empty() || !seen.insert(&item.msg_id) {
            return Err(invalid(
                topic,
                "deleteMessages contains an empty or duplicate msgId",
            ));
        }
        if pull_ids.contains(&item.msg_id) {
            return Err(invalid(
                topic,
                "the same message cannot be pulled and deleted in one decision",
            ));
        }
        if !(0..=MAX_SAFE_JSON_INTEGER).contains(&item.deleted_at) {
            return Err(invalid(topic, "deleteMessages requires a safe deletedAt"));
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
