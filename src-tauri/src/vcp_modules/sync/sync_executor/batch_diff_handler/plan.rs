use super::decision::{parse_topic_decision, TopicDecision};
use super::error::Phase3ProtocolError;
use super::protocol::{MAX_PHASE3_MESSAGES, MAX_PHASE3_TOPICS};
use crate::vcp_modules::sync_types::{MessageDeleteDecision, MessageDiffDecision};
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::HashSet;

pub(crate) struct BatchPlan {
    pub(crate) push_topics: Vec<TopicKey>,
    pub(crate) pull_batch: Vec<(TopicKey, Vec<String>)>,
    pub(crate) delete_batch: Vec<(TopicKey, Vec<MessageDeleteDecision>)>,
    pub(crate) idle_topics: Vec<TopicKey>,
    pub(crate) active_topics: HashSet<TopicKey>,
}

pub(crate) fn validate_phase3_result_topics<'a>(
    expected: &HashSet<TopicKey>,
    results: &'a [MessageDiffDecision],
) -> Result<Vec<(TopicKey, &'a MessageDiffDecision)>, String> {
    if results.len() > MAX_PHASE3_TOPICS {
        return Err(format!(
            "Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"
        ));
    }
    let mut keyed_results = Vec::with_capacity(results.len());
    let mut actual = HashSet::new();
    for result in results {
        let key = TopicKey::new(
            result.owner_type.as_str(),
            &result.owner_id,
            &result.topic_id,
        );
        if !key.is_valid() {
            return Err("SYNC_MESSAGE_DIFF_RESULT contains an invalid topic identity".into());
        }
        if !actual.insert(key.clone()) {
            return Err("Phase 3 response contains a duplicate topic identity".to_string());
        }
        keyed_results.push((key, result));
    }
    if actual == *expected {
        return Ok(keyed_results);
    }
    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let mut unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "Phase 3 response topic mismatch: missing={missing:?}, unexpected={unexpected:?}"
    ))
}

pub(crate) fn build_plan(
    keyed_results: Vec<(TopicKey, &MessageDiffDecision)>,
) -> Result<BatchPlan, Phase3ProtocolError> {
    let mut plan = BatchPlan {
        push_topics: Vec::new(),
        pull_batch: Vec::new(),
        delete_batch: Vec::new(),
        idle_topics: Vec::new(),
        active_topics: HashSet::new(),
    };
    let mut operation_count = 0usize;
    for (topic, result) in keyed_results {
        let decision = parse_topic_decision(&topic, result)?;
        add_decision(&mut plan, topic, decision, &mut operation_count)?;
    }
    Ok(plan)
}

fn add_decision(
    plan: &mut BatchPlan,
    topic: TopicKey,
    decision: TopicDecision,
    operation_count: &mut usize,
) -> Result<(), Phase3ProtocolError> {
    let count = decision
        .pull_message_ids
        .len()
        .checked_add(decision.delete_messages.len())
        .ok_or_else(|| budget_error(&topic, "message operation count overflow"))?;
    *operation_count = operation_count
        .checked_add(count)
        .ok_or_else(|| budget_error(&topic, "message operation count overflow"))?;
    if *operation_count > MAX_PHASE3_MESSAGES {
        return Err(budget_error(
            &topic,
            &format!("message operation count exceeds {MAX_PHASE3_MESSAGES}"),
        ));
    }
    if !decision.push_topic
        && decision.pull_message_ids.is_empty()
        && decision.delete_messages.is_empty()
    {
        plan.idle_topics.push(topic);
        return Ok(());
    }
    plan.active_topics.insert(topic.clone());
    if decision.push_topic {
        plan.push_topics.push(topic.clone());
    }
    if !decision.pull_message_ids.is_empty() {
        plan.pull_batch
            .push((topic.clone(), decision.pull_message_ids));
    }
    if !decision.delete_messages.is_empty() {
        plan.delete_batch.push((topic, decision.delete_messages));
    }
    Ok(())
}

fn budget_error(topic: &TopicKey, detail: &str) -> Phase3ProtocolError {
    Phase3ProtocolError::for_topic(
        "PHASE3_DECISION_BUDGET_EXCEEDED",
        format!("Phase 3 decision for {} {detail}", topic.topic_id),
        &topic.topic_id,
    )
}
