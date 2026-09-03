use super::common::{
    deserialize_content_hash, deserialize_non_empty_string, deserialize_sha256,
    deserialize_timestamp, is_sha256, serialize_timestamp, MAX_MESSAGES_PER_TOPIC,
    MAX_MESSAGE_DIFF_ITEMS, MAX_MESSAGE_DIFF_TOPICS, MAX_SAFE_TIMESTAMP, MAX_TOPIC_DIFF_ITEMS,
};
use super::transport::OwnerType;
use crate::vcp_modules::sync_error::WireSyncError;
use crate::vcp_modules::topic_types::TopicKey;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn deserialize_optional_bounded_strings<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Option::<Vec<String>>::deserialize(deserializer)?;
    let Some(values) = values else {
        return Ok(None);
    };
    if values.len() > MAX_MESSAGE_DIFF_ITEMS || values.iter().any(String::is_empty) {
        return Err(D::Error::custom(
            "message identity list exceeds its budget or contains an empty id",
        ));
    }
    let mut seen = BTreeSet::new();
    if values.iter().any(|value| !seen.insert(value)) {
        return Err(D::Error::custom(
            "message identity list contains duplicates",
        ));
    }
    Ok(Some(values))
}

fn deserialize_optional_delete_decisions<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<MessageDeleteDecision>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Option::<Vec<MessageDeleteDecision>>::deserialize(deserializer)?;
    let Some(values) = values else {
        return Ok(None);
    };
    if values.len() > MAX_MESSAGE_DIFF_ITEMS {
        return Err(D::Error::custom("delete decision count exceeds its budget"));
    }
    let mut seen = BTreeSet::new();
    if values
        .iter()
        .any(|value| !seen.insert(value.msg_id.as_str()))
    {
        return Err(D::Error::custom("delete decisions contain duplicate ids"));
    }
    Ok(Some(values))
}

fn deserialize_bounded_message_map<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, MessageVersionState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = BTreeMap::<String, MessageVersionState>::deserialize(deserializer)?;
    if values.len() > MAX_MESSAGES_PER_TOPIC {
        return Err(D::Error::custom(
            "message count exceeds the per-topic budget",
        ));
    }
    if values.keys().any(String::is_empty) {
        return Err(D::Error::custom("message identity must not be empty"));
    }
    Ok(values)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicDiffState {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    #[serde(deserialize_with = "deserialize_sha256")]
    pub config_hash: String,
    #[serde(deserialize_with = "deserialize_content_hash")]
    pub content_hash: String,
}

impl TopicDiffState {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.owner_type, OwnerType::Agent | OwnerType::Group)
            || self.owner_id.is_empty()
            || self.topic_id.is_empty()
            || !is_sha256(&self.config_hash, false)
            || !is_sha256(&self.content_hash, true)
        {
            return Err("topic diff requires a complete identity and valid hashes".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicDiffRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    pub topics: Vec<TopicDiffState>,
}

impl TopicDiffRequestFrame {
    pub fn new(topics: Vec<TopicDiffState>) -> Self {
        Self {
            frame_type: "SYNC_TOPIC_DIFF_REQUEST",
            topics,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.topics.len() > MAX_TOPIC_DIFF_ITEMS {
            return Err("topic diff exceeds the item budget".to_string());
        }
        let mut identities = BTreeSet::new();
        for topic in &self.topics {
            topic.validate()?;
            if !identities.insert(format!(
                "{}\0{}\0{}",
                topic.owner_type, topic.owner_id, topic.topic_id
            )) {
                return Err("topic diff contains duplicate identity".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicDiffResultFrame {
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_topic_diff_result_type"
    )]
    _frame_type: (),
    #[serde(deserialize_with = "deserialize_topic_keys")]
    pub changed_topics: Vec<TopicKey>,
}

fn deserialize_topic_diff_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_TOPIC_DIFF_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_TOPIC_DIFF_RESULT"))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum MessageVersionState {
    Live(MessageLiveState),
    Deleted(MessageDeletedState),
}

impl MessageVersionState {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Live(value)
                if is_sha256(&value.message_hash, false)
                    && (0..=MAX_SAFE_TIMESTAMP).contains(&value.updated_at) =>
            {
                Ok(())
            }
            Self::Deleted(value) if (0..=MAX_SAFE_TIMESTAMP).contains(&value.deleted_at) => Ok(()),
            _ => Err("message state has an invalid hash or timestamp".to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageLiveState {
    #[serde(deserialize_with = "deserialize_sha256")]
    pub message_hash: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeletedState {
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffTopicState {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    #[serde(deserialize_with = "deserialize_content_hash")]
    pub content_hash: String,
    #[serde(deserialize_with = "deserialize_bounded_message_map")]
    pub messages: BTreeMap<String, MessageVersionState>,
}

impl MessageDiffTopicState {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.owner_type, OwnerType::Agent | OwnerType::Group)
            || self.owner_id.is_empty()
            || self.topic_id.is_empty()
            || !is_sha256(&self.content_hash, true)
            || self.messages.len() > MAX_MESSAGES_PER_TOPIC
            || self.messages.keys().any(String::is_empty)
        {
            return Err("message diff requires a complete topic identity".to_string());
        }
        for state in self.messages.values() {
            state.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDiffRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    pub topics: Vec<MessageDiffTopicState>,
}

impl MessageDiffRequestFrame {
    pub fn new(topics: Vec<MessageDiffTopicState>) -> Self {
        Self {
            frame_type: "SYNC_MESSAGE_DIFF_REQUEST",
            topics,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.topics.len() > MAX_MESSAGE_DIFF_TOPICS {
            return Err("message diff topic count exceeds the budget".to_string());
        }
        let mut identities = BTreeSet::new();
        let mut message_count = 0usize;
        for topic in &self.topics {
            topic.validate()?;
            if !identities.insert(format!(
                "{}\0{}\0{}",
                topic.owner_type, topic.owner_id, topic.topic_id
            )) {
                return Err("message diff contains duplicate topic identity".to_string());
            }
            message_count = message_count.saturating_add(topic.messages.len());
        }
        if message_count > MAX_MESSAGE_DIFF_ITEMS {
            return Err("message diff exceeds the message budget".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeleteDecision {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub msg_id: String,
    #[serde(deserialize_with = "deserialize_timestamp")]
    pub deleted_at: i64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffDecision {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    pub ok: bool,
    #[serde(default, deserialize_with = "deserialize_optional_bounded_strings")]
    pub pull_message_ids: Option<Vec<String>>,
    #[serde(default)]
    pub push_topic: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_delete_decisions")]
    pub delete_messages: Option<Vec<MessageDeleteDecision>>,
    #[serde(default)]
    pub error: Option<WireSyncError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDiffResultFrame {
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_message_diff_result_type"
    )]
    _frame_type: (),
    #[serde(deserialize_with = "deserialize_message_diff_decisions")]
    pub results: Vec<MessageDiffDecision>,
}

fn deserialize_message_diff_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_MESSAGE_DIFF_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_MESSAGE_DIFF_RESULT"))
    }
}

fn deserialize_topic_keys<'de, D>(deserializer: D) -> Result<Vec<TopicKey>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<TopicKey>::deserialize(deserializer)?;
    if values.len() > MAX_TOPIC_DIFF_ITEMS {
        return Err(D::Error::custom("changed topic count exceeds the budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        if !value.is_valid()
            || !identities.insert(format!(
                "{}\0{}\0{}",
                value.owner_type, value.owner_id, value.topic_id
            ))
        {
            return Err(D::Error::custom(
                "changed topics require unique complete identities",
            ));
        }
    }
    Ok(values)
}

fn deserialize_message_diff_decisions<'de, D>(
    deserializer: D,
) -> Result<Vec<MessageDiffDecision>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<MessageDiffDecision>::deserialize(deserializer)?;
    if values.len() > MAX_MESSAGE_DIFF_TOPICS {
        return Err(D::Error::custom(
            "message diff topic count exceeds the budget",
        ));
    }
    let mut identities = BTreeSet::new();
    let mut message_count = 0usize;
    for value in &values {
        let identity = format!(
            "{}\0{}\0{}",
            value.owner_type, value.owner_id, value.topic_id
        );
        if !identities.insert(identity) {
            return Err(D::Error::custom(
                "message diff results contain duplicate topic identity",
            ));
        }
        if let Some(ids) = &value.pull_message_ids {
            message_count = message_count.saturating_add(ids.len());
        }
        if let Some(ids) = &value.delete_messages {
            message_count = message_count.saturating_add(ids.len());
        }
    }
    if message_count > MAX_MESSAGE_DIFF_ITEMS {
        return Err(D::Error::custom(
            "message diff result exceeds the message budget",
        ));
    }
    Ok(values)
}
