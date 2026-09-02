use super::error::Phase3ProtocolError;
use crate::vcp_modules::sync::wire_protocol::parse_strict_json;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use std::fmt;

pub(crate) const MAX_PHASE3_TOPICS: usize = 10_000;
pub(crate) const MAX_PHASE3_MESSAGES: usize = 100_000;

struct UniqueResults(Map<String, Value>);

impl<'de> Deserialize<'de> for UniqueResults {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueResultsVisitor;

        impl<'de> Visitor<'de> for UniqueResultsVisitor {
            type Value = UniqueResults;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with unique topic ids")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut results = Map::new();
                while let Some((topic_id, value)) = access.next_entry::<String, Value>()? {
                    if results.insert(topic_id.clone(), value).is_some() {
                        return Err(de::Error::custom(format!(
                            "duplicate Phase 3 topic id {topic_id}"
                        )));
                    }
                }
                Ok(UniqueResults(results))
            }
        }

        deserializer.deserialize_map(UniqueResultsVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Phase3BatchWire {
    #[serde(rename = "type")]
    message_type: String,
    results: UniqueResults,
}

pub fn parse_phase3_batch_frame(text: &str) -> Result<Value, Phase3ProtocolError> {
    let strict = parse_strict_json(text).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid Phase 3 batch frame: {error}"),
        )
    })?;
    let wire: Phase3BatchWire = serde_json::from_value(strict).map_err(|error| {
        Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            format!("Invalid Phase 3 batch frame: {error}"),
        )
    })?;
    if wire.message_type != "SYNC_DIFF_RESULTS_BATCH" {
        return Err(Phase3ProtocolError::new(
            "PHASE3_FRAME_INVALID",
            "Phase 3 batch frame has an unexpected type",
        ));
    }
    if wire.results.0.len() > MAX_PHASE3_TOPICS {
        return Err(Phase3ProtocolError::new(
            "PHASE3_DECISION_BUDGET_EXCEEDED",
            format!("Phase 3 response exceeds {MAX_PHASE3_TOPICS} topic budget"),
        ));
    }
    Ok(json!({
        "type": wire.message_type,
        "results": wire.results.0,
    }))
}
