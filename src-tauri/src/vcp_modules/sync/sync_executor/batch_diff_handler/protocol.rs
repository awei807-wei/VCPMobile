use super::error::Phase3ProtocolError;
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use crate::vcp_modules::sync_types::MessageDiffResultFrame;

pub(crate) const MAX_PHASE3_TOPICS: usize = 10_000;
pub(crate) const MAX_PHASE3_MESSAGES_PER_TOPIC: usize = 10_000;
pub(crate) const MAX_PHASE3_MESSAGES: usize = 100_000;
pub(crate) const MAX_SAFE_JSON_INTEGER: i64 = (1_i64 << 53) - 1;

/// Parse only the Wire 1.4 message-diff result frame.
pub fn parse_message_diff_result_frame(
    text: &str,
) -> Result<MessageDiffResultFrame, Phase3ProtocolError> {
    let value = parse_strict_json(text).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid SYNC_MESSAGE_DIFF_RESULT frame: {error}"),
        )
    })?;
    let frame = serde_json::from_value::<MessageDiffResultFrame>(value).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid SYNC_MESSAGE_DIFF_RESULT frame: {error}"),
        )
    })?;
    if frame.results.len() > MAX_PHASE3_TOPICS {
        return Err(Phase3ProtocolError::new(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!("Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"),
        ));
    }
    Ok(frame)
}
