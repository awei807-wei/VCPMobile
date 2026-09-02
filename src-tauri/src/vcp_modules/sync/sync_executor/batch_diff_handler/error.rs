use crate::vcp_modules::sync_error::{encode_wire_sync_error, WireSyncError};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase3ProtocolError {
    pub code: String,
    pub message: String,
    pub failed_topic_ids: Vec<String>,
}

impl Phase3ProtocolError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            failed_topic_ids: Vec::new(),
        }
    }

    pub(crate) fn for_topic(
        code: impl Into<String>,
        message: impl Into<String>,
        topic_id: &str,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            failed_topic_ids: vec![topic_id.to_string()],
        }
    }

    pub(crate) fn from_wire(wire: WireSyncError, topic_id: &str) -> Result<Self, Self> {
        let code = wire.code.clone();
        let mut failed_topic_ids = wire.failed_topic_ids.clone();
        if !failed_topic_ids.iter().any(|id| id == topic_id) {
            failed_topic_ids.truncate(7);
            failed_topic_ids.push(topic_id.to_string());
        }
        failed_topic_ids.truncate(8);
        let message = encode_wire_sync_error(&wire).map_err(|error| {
            Self::for_topic(
                "PHASE3_DECISION_INVALID",
                format!("Phase 3 rejection for {topic_id} has invalid error: {error}"),
                topic_id,
            )
        })?;
        Ok(Self {
            code,
            message,
            failed_topic_ids,
        })
    }
}

impl fmt::Display for Phase3ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Phase3ProtocolError {}
