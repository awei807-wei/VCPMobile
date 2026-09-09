use super::common::{
    deserialize_content_hash, deserialize_non_empty_string, deserialize_sha256,
    deserialize_timestamp, serialize_timestamp, MAX_MANIFEST_ITEMS,
};
use super::transport::{is_valid_avatar_owner, AvatarOwnerType, OwnerType};
use crate::vcp_modules::topic_types::OwnerKey;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum OwnerManifestState {
    Live(OwnerManifestLive),
    Deleted(OwnerManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestLive {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_sha256")]
    pub config_hash: String,
    #[serde(deserialize_with = "deserialize_content_hash")]
    pub content_hash: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestDeleted {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum TopicManifestState {
    Live(TopicManifestLive),
    Deleted(TopicManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestLive {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    #[serde(deserialize_with = "deserialize_sha256")]
    pub config_hash: String,
    #[serde(deserialize_with = "deserialize_content_hash")]
    pub content_hash: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestDeleted {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub deleted_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum AvatarManifestState {
    Live(AvatarManifestLive),
    Deleted(AvatarManifestDeleted),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestLive {
    pub owner_type: AvatarOwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_sha256")]
    pub binary_hash: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestDeleted {
    pub owner_type: AvatarOwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(
        deserialize_with = "deserialize_timestamp",
        serialize_with = "serialize_timestamp"
    )]
    pub deleted_at: i64,
}

fn owner_manifest_identity(state: &OwnerManifestState) -> String {
    match state {
        OwnerManifestState::Live(value) => {
            format!("{}\0{}", value.owner_type, value.owner_id)
        }
        OwnerManifestState::Deleted(value) => {
            format!("{}\0{}", value.owner_type, value.owner_id)
        }
    }
}

fn topic_manifest_identity(state: &TopicManifestState) -> String {
    match state {
        TopicManifestState::Live(value) => format!(
            "{}\0{}\0{}",
            value.owner_type, value.owner_id, value.topic_id
        ),
        TopicManifestState::Deleted(value) => format!(
            "{}\0{}\0{}",
            value.owner_type, value.owner_id, value.topic_id
        ),
    }
}

fn avatar_manifest_identity(state: &AvatarManifestState) -> String {
    match state {
        AvatarManifestState::Live(value) => format!("{}\0{}", value.owner_type, value.owner_id),
        AvatarManifestState::Deleted(value) => {
            format!("{}\0{}", value.owner_type, value.owner_id)
        }
    }
}

fn validate_owner_manifest_state(state: &OwnerManifestState) -> Result<(), String> {
    let (owner_type, owner_id) = match state {
        OwnerManifestState::Live(value) => (value.owner_type, value.owner_id.as_str()),
        OwnerManifestState::Deleted(value) => (value.owner_type, value.owner_id.as_str()),
    };
    if owner_id.is_empty() {
        return Err("owner manifest requires complete owner identity".to_string());
    }
    if !matches!(owner_type, OwnerType::Agent | OwnerType::Group) {
        return Err("owner manifest requires agent/group ownerType".to_string());
    }
    Ok(())
}

fn validate_topic_manifest_state(state: &TopicManifestState) -> Result<(), String> {
    let (owner_type, owner_id, topic_id) = match state {
        TopicManifestState::Live(value) => (
            value.owner_type,
            value.owner_id.as_str(),
            value.topic_id.as_str(),
        ),
        TopicManifestState::Deleted(value) => (
            value.owner_type,
            value.owner_id.as_str(),
            value.topic_id.as_str(),
        ),
    };
    if !matches!(owner_type, OwnerType::Agent | OwnerType::Group)
        || owner_id.is_empty()
        || topic_id.is_empty()
    {
        return Err("topic manifest requires complete compound identity".to_string());
    }
    Ok(())
}

fn validate_avatar_manifest_state(state: &AvatarManifestState) -> Result<(), String> {
    let (owner_type, owner_id) = match state {
        AvatarManifestState::Live(value) => (value.owner_type, value.owner_id.as_str()),
        AvatarManifestState::Deleted(value) => (value.owner_type, value.owner_id.as_str()),
    };
    if !is_valid_avatar_owner(owner_type.as_str(), owner_id) {
        return Err("avatar manifest requires a valid avatar identity".to_string());
    }
    Ok(())
}

pub(crate) fn deserialize_owner_manifest_states<'de, D>(
    deserializer: D,
) -> Result<Vec<OwnerManifestState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<OwnerManifestState>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("owner manifest exceeds the item budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        validate_owner_manifest_state(value).map_err(D::Error::custom)?;
        if !identities.insert(owner_manifest_identity(value)) {
            return Err(D::Error::custom(
                "owner manifest contains duplicate identity",
            ));
        }
    }
    Ok(values)
}

pub(crate) fn deserialize_topic_manifest_states<'de, D>(
    deserializer: D,
) -> Result<Vec<TopicManifestState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<TopicManifestState>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("topic manifest exceeds the item budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        validate_topic_manifest_state(value).map_err(D::Error::custom)?;
        if !identities.insert(topic_manifest_identity(value)) {
            return Err(D::Error::custom(
                "topic manifest contains duplicate identity",
            ));
        }
    }
    Ok(values)
}

pub(crate) fn deserialize_avatar_manifest_states<'de, D>(
    deserializer: D,
) -> Result<Vec<AvatarManifestState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<AvatarManifestState>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("avatar manifest exceeds the item budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        validate_avatar_manifest_state(value).map_err(D::Error::custom)?;
        if !identities.insert(avatar_manifest_identity(value)) {
            return Err(D::Error::custom(
                "avatar manifest contains duplicate identity",
            ));
        }
    }
    Ok(values)
}

pub(crate) fn deserialize_targeted_owners<'de, D>(
    deserializer: D,
) -> Result<Vec<OwnerKey>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<OwnerKey>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("targetedOwners exceeds the item budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        if !value.is_valid()
            || !identities.insert(format!("{}\0{}", value.owner_type, value.owner_id))
        {
            return Err(D::Error::custom(
                "targetedOwners requires unique agent/group identities",
            ));
        }
    }
    Ok(values)
}

pub(crate) fn validate_owner_manifest_items(items: &[OwnerManifestState]) -> Result<(), String> {
    if items.len() > MAX_MANIFEST_ITEMS {
        return Err("owner manifest exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for value in items {
        validate_owner_manifest_state(value)?;
        if !identities.insert(owner_manifest_identity(value)) {
            return Err("owner manifest contains duplicate identity".to_string());
        }
    }
    Ok(())
}

pub(crate) fn validate_topic_manifest_items(items: &[TopicManifestState]) -> Result<(), String> {
    if items.len() > MAX_MANIFEST_ITEMS {
        return Err("topic manifest exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for value in items {
        validate_topic_manifest_state(value)?;
        if !identities.insert(topic_manifest_identity(value)) {
            return Err("topic manifest contains duplicate identity".to_string());
        }
    }
    Ok(())
}

pub(crate) fn validate_avatar_manifest_items(items: &[AvatarManifestState]) -> Result<(), String> {
    if items.len() > MAX_MANIFEST_ITEMS {
        return Err("avatar manifest exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for value in items {
        validate_avatar_manifest_state(value)?;
        if !identities.insert(avatar_manifest_identity(value)) {
            return Err("avatar manifest contains duplicate identity".to_string());
        }
    }
    Ok(())
}

pub(crate) fn validate_targeted_owners(owners: &[OwnerKey]) -> Result<(), String> {
    if owners.len() > MAX_MANIFEST_ITEMS {
        return Err("targetedOwners exceeds the item budget".to_string());
    }
    let mut identities = BTreeSet::new();
    for owner in owners {
        if !owner.is_valid()
            || !identities.insert(format!("{}\0{}", owner.owner_type, owner.owner_id))
        {
            return Err("targetedOwners requires unique agent/group identities".to_string());
        }
    }
    Ok(())
}
