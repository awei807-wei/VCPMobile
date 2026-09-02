use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct TopicBatchOutcome {
    pub(crate) topic_id: String,
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn validate_topic_batch_outcomes(
    operation: &str,
    expected: &[String],
    batch_result: Result<Vec<TopicBatchOutcome>, String>,
) -> Result<Vec<String>, String> {
    let outcomes = batch_result.map_err(|error| batch_error(operation, expected, error))?;
    let expected_set = expected.iter().cloned().collect::<HashSet<_>>();
    let mut outcomes_by_topic = HashMap::new();
    for outcome in outcomes {
        if !expected_set.contains(&outcome.topic_id) {
            return Err(format!(
                "Phase 3 {operation} response contains unexpected topic {}",
                outcome.topic_id
            ));
        }
        if outcomes_by_topic
            .insert(outcome.topic_id.clone(), outcome)
            .is_some()
        {
            return Err(format!(
                "Phase 3 {operation} response contains duplicate topic"
            ));
        }
    }

    let mut successful = Vec::new();
    let mut failed = Vec::new();
    for topic_id in expected {
        match outcomes_by_topic.get(topic_id) {
            Some(outcome) if outcome.success => successful.push(topic_id.clone()),
            Some(outcome) => failed.push(format!(
                "{}: {}",
                topic_id,
                outcome.error.as_deref().unwrap_or("unknown error")
            )),
            None => failed.push(format!("{}: missing from batch response", topic_id)),
        }
    }
    if failed.is_empty() {
        Ok(successful)
    } else {
        failed.sort();
        Err(format!(
            "Phase 3 {operation} failed topics: {}",
            failed.join(", ")
        ))
    }
}

fn batch_error(operation: &str, expected: &[String], error: String) -> String {
    let mut topics = expected.to_vec();
    topics.sort();
    format!(
        "Phase 3 {operation} batch failed for topics {:?}: {}",
        topics, error
    )
}

pub(crate) fn validate_phase3_result_topics(
    expected: &HashSet<String>,
    results: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let actual: HashSet<String> = results.keys().cloned().collect();
    if actual == *expected {
        return Ok(());
    }
    let mut missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let mut unexpected = actual.difference(expected).cloned().collect::<Vec<_>>();
    missing.sort();
    unexpected.sort();
    Err(format!(
        "Phase 3 response topic mismatch: missing={:?}, unexpected={:?}",
        missing, unexpected
    ))
}
