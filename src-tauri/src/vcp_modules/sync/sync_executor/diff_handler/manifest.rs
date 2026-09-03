use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::{
    is_valid_avatar_owner, AvatarManifestDecision, DeleteTarget, ManifestAction,
    ManifestResultFrame, ManifestType, OwnerManifestDecision, OwnerType, TopicManifestDecision,
};
use crate::vcp_modules::topic_types::{OwnerKey, TopicKey};
use std::collections::{BTreeSet, HashSet};
use std::sync::Mutex;

/// A manifest decision with its identity tied to the manifest namespace.
///
/// Keeping the variants separate prevents an owner id or topic id from being
/// accidentally reused without its namespace when a decision is dispatched.
#[derive(Debug, Clone)]
pub(crate) enum ManifestDecision {
    Owner(OwnerManifestDecision),
    Topic(TopicManifestDecision),
    Avatar(AvatarManifestDecision),
}

impl ManifestDecision {
    pub(crate) fn action(&self) -> ManifestAction {
        match self {
            Self::Owner(item) => item.action,
            Self::Topic(item) => item.action,
            Self::Avatar(item) => item.action,
        }
    }

    pub(crate) fn identity(&self) -> String {
        match self {
            Self::Owner(item) => format!("owner\0{}\0{}", item.owner_type, item.owner_id),
            Self::Topic(item) => format!(
                "topic\0{}\0{}\0{}",
                item.owner_type, item.owner_id, item.topic_id
            ),
            Self::Avatar(item) => format!("avatar\0{}\0{}", item.owner_type, item.owner_id),
        }
    }

    pub(crate) fn display_id(&self) -> &str {
        match self {
            Self::Owner(item) => &item.owner_id,
            Self::Topic(item) => &item.topic_id,
            Self::Avatar(item) => &item.owner_id,
        }
    }

    pub(crate) fn deleted_at(&self) -> Option<i64> {
        match self {
            Self::Owner(item) => item.deleted_at,
            Self::Topic(item) => item.deleted_at,
            Self::Avatar(item) => item.deleted_at,
        }
    }

    pub(crate) fn owner_key(&self) -> Option<OwnerKey> {
        match self {
            Self::Owner(item) => Some(OwnerKey::new(item.owner_type.as_str(), &item.owner_id)),
            Self::Topic(_) | Self::Avatar(_) => None,
        }
    }

    pub(crate) fn topic_key(&self) -> Option<TopicKey> {
        match self {
            Self::Topic(item) => Some(TopicKey::new(
                item.owner_type.as_str(),
                &item.owner_id,
                &item.topic_id,
            )),
            Self::Owner(_) | Self::Avatar(_) => None,
        }
    }

    pub(crate) fn delete_target(&self) -> DeleteTarget {
        match self {
            Self::Owner(item) => DeleteTarget::Owner {
                owner_type: item.owner_type,
                owner_id: item.owner_id.clone(),
            },
            Self::Topic(item) => DeleteTarget::Topic(TopicKey::new(
                item.owner_type.as_str(),
                &item.owner_id,
                &item.topic_id,
            )),
            Self::Avatar(item) => DeleteTarget::Avatar {
                owner_type: item.owner_type,
                owner_id: item.owner_id.clone(),
            },
        }
    }
}

fn validate_decision_timestamp(decision: &ManifestDecision) -> Result<(), String> {
    let action = decision.action();
    let deleted_at = decision.deleted_at();
    let is_valid = deleted_at.is_some_and(|value| (0..=9_007_199_254_740_991_i64).contains(&value));
    if action.is_delete() && !is_valid {
        return Err(format!(
            "SYNC_MANIFEST_RESULT {} {} requires a non-negative safe-integer deletedAt",
            action,
            decision.display_id()
        ));
    }
    if !action.is_delete() && deleted_at.is_some() {
        return Err(format!(
            "SYNC_MANIFEST_RESULT {} {} must not carry deletedAt",
            action,
            decision.display_id()
        ));
    }
    Ok(())
}

fn validate_owner_decision(item: &OwnerManifestDecision) -> Result<(), String> {
    if item.owner_id.is_empty() {
        return Err("SYNC_MANIFEST_RESULT contains an empty owner identity".to_string());
    }
    if !matches!(item.owner_type, OwnerType::Agent | OwnerType::Group) {
        return Err("SYNC_MANIFEST_RESULT contains an invalid ownerType".to_string());
    }
    if item.content_hash_mismatch && item.action.is_delete() {
        return Err(format!(
            "Owner {} delete decision must not report contentHashMismatch",
            item.owner_id
        ));
    }
    if item.action == ManifestAction::Skip && !item.content_hash_mismatch {
        return Err(format!(
            "Owner {} SKIP decision requires contentHashMismatch",
            item.owner_id
        ));
    }
    Ok(())
}

fn validate_topic_decision(item: &TopicManifestDecision) -> Result<(), String> {
    if item.owner_id.is_empty() || item.topic_id.is_empty() {
        return Err("SYNC_MANIFEST_RESULT contains an invalid topic identity".to_string());
    }
    if item.action == ManifestAction::Skip {
        return Err("Topic manifest must not contain SKIP decisions".to_string());
    }
    Ok(())
}

fn validate_avatar_decision(item: &AvatarManifestDecision) -> Result<(), String> {
    if !is_valid_avatar_owner(item.owner_type.as_str(), &item.owner_id) {
        return Err("SYNC_MANIFEST_RESULT contains an invalid avatar identity".to_string());
    }
    if item.action == ManifestAction::Skip {
        return Err("Avatar manifest must not contain SKIP decisions".to_string());
    }
    Ok(())
}

/// Decode a typed Wire 1.4 result and validate its identities and action.
pub(crate) fn validate_manifest_result(
    result: ManifestResultFrame,
) -> Result<(ManifestType, Vec<ManifestDecision>), String> {
    let (manifest_type, decisions) = match result {
        ManifestResultFrame::Owner { results, .. } => (
            ManifestType::Owner,
            results.into_iter().map(ManifestDecision::Owner).collect(),
        ),
        ManifestResultFrame::Topic { results, .. } => (
            ManifestType::Topic,
            results.into_iter().map(ManifestDecision::Topic).collect(),
        ),
        ManifestResultFrame::Avatar { results, .. } => (
            ManifestType::Avatar,
            results.into_iter().map(ManifestDecision::Avatar).collect(),
        ),
    };
    let mut seen = BTreeSet::new();
    for decision in &decisions {
        validate_decision_timestamp(decision)?;
        match decision {
            ManifestDecision::Owner(item) => validate_owner_decision(item)?,
            ManifestDecision::Topic(item) => validate_topic_decision(item)?,
            ManifestDecision::Avatar(item) => validate_avatar_decision(item)?,
        }
        if !seen.insert(decision.identity()) {
            return Err(format!(
                "SYNC_MANIFEST_RESULT contains a duplicate {manifest_type} identity"
            ));
        }
    }
    Ok((manifest_type, decisions))
}

/// Mark a manifest type as received exactly once for the current phase.
pub(crate) fn consume_manifest_response_type(
    manifest_type: ManifestType,
    current_wave: u8,
    expected_manifest_types: &Mutex<HashSet<ManifestType>>,
) -> Result<bool, String> {
    let mut remaining = expected_manifest_types
        .lock()
        .map_err(|_| "Expected manifest type set is poisoned".to_string())?;
    if !remaining.remove(&manifest_type) {
        return Err(format!(
            "SYNC_MANIFEST_RESULT contains duplicate or unexpected manifestType {manifest_type} for wave {current_wave}"
        ));
    }
    Ok(remaining.is_empty())
}

pub(crate) fn next_manifest_command(current_phase: u8, attempt_id: u64) -> Option<SyncCommand> {
    match current_phase {
        1 => Some(SyncCommand::StartAvatarMetadata { attempt_id }),
        2 => Some(SyncCommand::StartTopicMetadata { attempt_id }),
        3 => Some(SyncCommand::StartTopicValidation { attempt_id }),
        _ => None,
    }
}
