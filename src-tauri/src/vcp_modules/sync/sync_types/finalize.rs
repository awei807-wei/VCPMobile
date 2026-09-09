use super::common::serialize_safe_u64;
use serde::de::Error as _;
use serde::ser::{Error as _, SerializeStruct};
use serde::{Deserialize, Serialize};
use std::fmt;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Manifest 仲裁动作。
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ManifestAction {
    Pull,
    Push,
    PullDelete,
    PushDelete,
    Skip,
}

impl ManifestAction {
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestAction::Pull => "PULL",
            ManifestAction::Push => "PUSH",
            ManifestAction::PullDelete => "PULL_DELETE",
            ManifestAction::PushDelete => "PUSH_DELETE",
            ManifestAction::Skip => "SKIP",
        }
    }

    pub fn is_delete(self) -> bool {
        matches!(
            self,
            ManifestAction::PullDelete | ManifestAction::PushDelete
        )
    }
}

impl fmt::Display for ManifestAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    OwnerMetadata,
    TopicMetadata,
    Messages,
}

impl SyncPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncPhase::OwnerMetadata => "owner_metadata",
            SyncPhase::TopicMetadata => "topic_metadata",
            SyncPhase::Messages => "messages",
        }
    }
}

impl fmt::Display for SyncPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Per-phase counters emitted by the synchronization progress layer.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PhaseStats {
    pub phase: SyncPhase,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub expected: u64,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub success: u64,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub errors: u64,
    #[serde(
        deserialize_with = "deserialize_safe_u64",
        serialize_with = "serialize_safe_u64"
    )]
    pub duration_ms: u64,
}

impl PhaseStats {
    pub fn validate(&self) -> Result<(), String> {
        if self.success.saturating_add(self.errors) > self.expected {
            return Err("phase statistics exceed the expected count".to_string());
        }
        Ok(())
    }
}

pub type SyncPhaseStats = PhaseStats;

/// The final messages acknowledgement has an exact five-field wire shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalAckFrame {
    pub phase: SyncPhase,
    pub session_id: u64,
    pub attempt_id: u64,
    pub nonce: String,
}

impl FinalAckFrame {
    pub fn new(session_id: u64, attempt_id: u64, nonce: impl Into<String>) -> Self {
        Self {
            phase: SyncPhase::Messages,
            session_id,
            attempt_id,
            nonce: nonce.into(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.phase != SyncPhase::Messages
            || self.session_id > MAX_SAFE_INTEGER
            || self.attempt_id > MAX_SAFE_INTEGER
            || self.nonce.is_empty()
        {
            return Err("final acknowledgement requires messages identity".to_string());
        }
        Ok(())
    }
}

impl Serialize for FinalAckFrame {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(S::Error::custom)?;
        let mut state = serializer.serialize_struct("FinalAckFrame", 5)?;
        state.serialize_field("type", "PHASE_ACK")?;
        state.serialize_field("phase", &self.phase)?;
        state.serialize_field("sessionId", &self.session_id)?;
        state.serialize_field("attemptId", &self.attempt_id)?;
        state.serialize_field("nonce", &self.nonce)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for FinalAckFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Wire {
            #[serde(rename = "type", deserialize_with = "deserialize_phase_ack_type")]
            _frame_type: (),
            phase: SyncPhase,
            #[serde(deserialize_with = "deserialize_safe_u64")]
            session_id: u64,
            #[serde(deserialize_with = "deserialize_safe_u64")]
            attempt_id: u64,
            nonce: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let frame = Self {
            phase: wire.phase,
            session_id: wire.session_id,
            attempt_id: wire.attempt_id,
            nonce: wire.nonce,
        };
        frame.validate().map_err(D::Error::custom)?;
        Ok(frame)
    }
}

fn deserialize_safe_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    if value > MAX_SAFE_INTEGER {
        return Err(D::Error::custom("value must be a JavaScript safe integer"));
    }
    Ok(value)
}

pub type FinalPhaseAck = FinalAckFrame;

fn deserialize_phase_ack_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "PHASE_ACK" {
        Ok(())
    } else {
        Err(D::Error::custom("expected PHASE_ACK"))
    }
}
