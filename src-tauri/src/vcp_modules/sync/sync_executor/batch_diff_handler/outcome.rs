use crate::vcp_modules::sync_error::{decode_wire_sync_error, is_attempt_restart_code};
use crate::vcp_modules::topic_types::TopicKey;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct TopicBatchOutcome {
    pub(crate) topic: TopicKey,
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug)]
pub(crate) struct TopicBatchFailure {
    pub(crate) message: String,
    pub(crate) restart_code: Option<String>,
}

pub(crate) fn validate_topic_batch_outcomes(
    operation: &str,
    expected: &[TopicKey],
    batch_result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<Vec<TopicKey>, TopicBatchFailure> {
    let outcomes = batch_result.map_err(|error| batch_error(operation, expected, error))?;
    let expected_set = expected.iter().cloned().collect::<HashSet<_>>();
    let mut outcomes_by_topic = HashMap::new();
    for outcome in outcomes {
        if !expected_set.contains(&outcome.topic) {
            return Err(TopicBatchFailure {
                message: format!(
                    "Phase 3 {operation} response contains unexpected topic {}",
                    outcome.topic.topic_id
                ),
                restart_code: None,
            });
        }
        if outcomes_by_topic
            .insert(outcome.topic.clone(), outcome)
            .is_some()
        {
            return Err(TopicBatchFailure {
                message: format!("Phase 3 {operation} response contains duplicate topic"),
                restart_code: None,
            });
        }
    }
    collect_outcomes(operation, expected, outcomes_by_topic)
}

fn collect_outcomes(
    operation: &str,
    expected: &[TopicKey],
    outcomes: HashMap<TopicKey, TopicBatchOutcome>,
) -> Result<Vec<TopicKey>, TopicBatchFailure> {
    let mut successful = Vec::new();
    let mut failed = Vec::new();
    let mut restart_code = None;
    let mut all_failures_restartable = true;
    for topic in expected {
        match outcomes.get(topic) {
            Some(outcome) if outcome.success => successful.push(topic.clone()),
            Some(outcome) => {
                let detail = outcome.error.as_deref().unwrap_or("unknown error");
                let code = restart_code_for(detail);
                if let Some(code) = code {
                    restart_code.get_or_insert(code);
                } else {
                    all_failures_restartable = false;
                }
                failed.push(format!("{}: {detail}", topic.topic_id));
            }
            None => {
                all_failures_restartable = false;
                failed.push(format!("{}: missing from batch response", topic.topic_id));
            }
        }
    }
    if failed.is_empty() {
        return Ok(successful);
    }
    failed.sort();
    Err(TopicBatchFailure {
        message: format!("Phase 3 {operation} failed topics: {}", failed.join(", ")),
        restart_code: all_failures_restartable.then_some(restart_code).flatten(),
    })
}

fn batch_error(operation: &str, expected: &[TopicKey], error: String) -> TopicBatchFailure {
    let mut topics = expected.to_vec();
    topics.sort();
    let restart_code = restart_code_for(&error);
    TopicBatchFailure {
        message: format!(
            "Phase 3 {operation} batch failed for topics {:?}: {error}",
            topics
        ),
        restart_code,
    }
}

fn restart_code_for(error: &str) -> Option<String> {
    decode_wire_sync_error(error)
        .filter(|wire| is_attempt_restart_code(&wire.code))
        .map(|wire| wire.code)
}
