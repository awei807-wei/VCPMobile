mod codec;
mod context;
mod decision;
mod error;
mod orchestration;
mod outcome;

pub struct BatchDiffHandler;
pub(crate) use context::{Phase3Context, Phase3ContextParams};

pub use codec::parse_phase3_batch_frame;
pub use error::Phase3ProtocolError;

#[cfg(test)]
pub(crate) use decision::{parse_topic_decision, TopicDecision};
#[cfg(test)]
pub(crate) use outcome::{
    validate_phase3_result_topics, validate_topic_batch_outcomes, TopicBatchOutcome,
};

impl BatchDiffHandler {
    pub async fn handle_diff_batch(
        context: Phase3Context,
        payload: &serde_json::Value,
    ) -> Result<(), Phase3ProtocolError> {
        orchestration::handle_diff_batch(&context, payload).await
    }
}

#[cfg(test)]
#[path = "batch_diff_handler_tests.rs"]
mod tests;
