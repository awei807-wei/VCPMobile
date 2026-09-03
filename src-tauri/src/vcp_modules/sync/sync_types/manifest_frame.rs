use super::common::{
    deserialize_non_empty_string, deserialize_optional_timestamp, MAX_MANIFEST_ITEMS,
};
use super::finalize::ManifestAction;
use super::manifest::{
    deserialize_avatar_manifest_states, deserialize_owner_manifest_states,
    deserialize_targeted_owners, deserialize_topic_manifest_states, validate_avatar_manifest_items,
    validate_owner_manifest_items, validate_targeted_owners, validate_topic_manifest_items,
    AvatarManifestState, OwnerManifestState, TopicManifestState,
};
use super::transport::{is_valid_avatar_owner, AvatarOwnerType, ManifestType, OwnerType};
use crate::vcp_modules::topic_types::OwnerKey;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn deserialize_owner_manifest_decisions<'de, D>(
    deserializer: D,
) -> Result<Vec<OwnerManifestDecision>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<OwnerManifestDecision>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("owner decision count exceeds the budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        let identity = format!("{}\0{}", value.owner_type, value.owner_id);
        if value.action.is_delete() != value.deleted_at.is_some() {
            return Err(D::Error::custom(
                "delete manifest actions require deletedAt and live actions must omit it",
            ));
        }
        if !identities.insert(identity) {
            return Err(D::Error::custom(
                "owner decisions contain duplicate identity",
            ));
        }
    }
    Ok(values)
}

fn deserialize_topic_manifest_decisions<'de, D>(
    deserializer: D,
) -> Result<Vec<TopicManifestDecision>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<TopicManifestDecision>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("topic decision count exceeds the budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        let identity = format!(
            "{}\0{}\0{}",
            value.owner_type, value.owner_id, value.topic_id
        );
        if value.action.is_delete() != value.deleted_at.is_some() {
            return Err(D::Error::custom(
                "delete manifest actions require deletedAt and live actions must omit it",
            ));
        }
        if !identities.insert(identity) {
            return Err(D::Error::custom(
                "topic decisions contain duplicate identity",
            ));
        }
    }
    Ok(values)
}

fn deserialize_avatar_manifest_decisions<'de, D>(
    deserializer: D,
) -> Result<Vec<AvatarManifestDecision>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<AvatarManifestDecision>::deserialize(deserializer)?;
    if values.len() > MAX_MANIFEST_ITEMS {
        return Err(D::Error::custom("avatar decision count exceeds the budget"));
    }
    let mut identities = BTreeSet::new();
    for value in &values {
        if value.action.is_delete() != value.deleted_at.is_some() {
            return Err(D::Error::custom(
                "delete manifest actions require deletedAt and live actions must omit it",
            ));
        }
        if !is_valid_avatar_owner(value.owner_type.as_str(), &value.owner_id)
            || !identities.insert(format!("{}\0{}", value.owner_type, value.owner_id))
        {
            return Err(D::Error::custom(
                "avatar decisions require unique valid identity",
            ));
        }
    }
    Ok(values)
}

/// Manifest 的三种条目在类型层分离，墓碑不再携带伪造的 live Hash。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "manifestType", rename_all = "lowercase", deny_unknown_fields)]
pub enum ManifestRequest {
    Owner {
        #[serde(deserialize_with = "deserialize_owner_manifest_states")]
        items: Vec<OwnerManifestState>,
    },
    Topic {
        #[serde(deserialize_with = "deserialize_topic_manifest_states")]
        items: Vec<TopicManifestState>,
        #[serde(
            rename = "targetedOwners",
            deserialize_with = "deserialize_targeted_owners"
        )]
        targeted_owners: Vec<OwnerKey>,
    },
    Avatar {
        #[serde(deserialize_with = "deserialize_avatar_manifest_states")]
        items: Vec<AvatarManifestState>,
    },
}

impl ManifestRequest {
    pub fn manifest_type(&self) -> ManifestType {
        match self {
            ManifestRequest::Owner { .. } => ManifestType::Owner,
            ManifestRequest::Topic { .. } => ManifestType::Topic,
            ManifestRequest::Avatar { .. } => ManifestType::Avatar,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Owner { items } => validate_owner_manifest_items(items),
            Self::Topic {
                items,
                targeted_owners,
            } => {
                validate_topic_manifest_items(items)?;
                validate_targeted_owners(targeted_owners)?;
                let targeted = targeted_owners
                    .iter()
                    .map(|owner| format!("{}\0{}", owner.owner_type, owner.owner_id))
                    .collect::<std::collections::BTreeSet<_>>();
                for item in items {
                    let identity = match item {
                        TopicManifestState::Live(value) => {
                            format!("{}\0{}", value.owner_type, value.owner_id)
                        }
                        TopicManifestState::Deleted(value) => {
                            format!("{}\0{}", value.owner_type, value.owner_id)
                        }
                    };
                    if !targeted.contains(&identity) {
                        return Err("topic manifest contains an untargeted owner".to_string());
                    }
                }
                Ok(())
            }
            Self::Avatar { items } => validate_avatar_manifest_items(items),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerManifestDecision {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_timestamp")]
    pub deleted_at: Option<i64>,
    #[serde(default)]
    pub content_hash_mismatch: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopicManifestDecision {
    pub owner_type: OwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub topic_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_timestamp")]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvatarManifestDecision {
    pub owner_type: AvatarOwnerType,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub owner_id: String,
    pub action: ManifestAction,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_optional_timestamp")]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "manifestType", rename_all = "lowercase", deny_unknown_fields)]
pub enum ManifestResultFrame {
    Owner {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        #[serde(deserialize_with = "deserialize_owner_manifest_decisions")]
        results: Vec<OwnerManifestDecision>,
    },
    Topic {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        #[serde(deserialize_with = "deserialize_topic_manifest_decisions")]
        results: Vec<TopicManifestDecision>,
    },
    Avatar {
        #[serde(rename = "type", deserialize_with = "deserialize_manifest_result_type")]
        _frame_type: (),
        #[serde(deserialize_with = "deserialize_avatar_manifest_decisions")]
        results: Vec<AvatarManifestDecision>,
    },
}

impl ManifestResultFrame {
    pub fn manifest_type(&self) -> ManifestType {
        match self {
            ManifestResultFrame::Owner { .. } => ManifestType::Owner,
            ManifestResultFrame::Topic { .. } => ManifestType::Topic,
            ManifestResultFrame::Avatar { .. } => ManifestType::Avatar,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ManifestRequestFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    #[serde(flatten)]
    pub manifest: ManifestRequest,
}

impl ManifestRequestFrame {
    pub fn new(manifest: ManifestRequest) -> Self {
        Self {
            frame_type: "SYNC_MANIFEST_REQUEST",
            manifest,
        }
    }
}

fn deserialize_manifest_result_type<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "SYNC_MANIFEST_RESULT" {
        Ok(())
    } else {
        Err(D::Error::custom("expected SYNC_MANIFEST_RESULT"))
    }
}
