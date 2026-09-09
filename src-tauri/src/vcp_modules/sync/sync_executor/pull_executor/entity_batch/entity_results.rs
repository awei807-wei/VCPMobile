use super::{
    non_empty_string, parse_owner_type, DecodedEntityResult, EntityIdentity, ValidEntityRequest,
};
use crate::vcp_modules::db_write_queue::DbWriteTask;
use crate::vcp_modules::sync::sync_error::parse_wire_sync_error;
use crate::vcp_modules::sync_dto::{
    AgentSyncDTO, AgentTopicSyncDTO, GroupSyncDTO, GroupTopicSyncDTO,
};
use crate::vcp_modules::sync_types::OwnerType;
use crate::vcp_modules::topic_types::TopicKey;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub(super) fn decode_entity_results(
    values: Vec<Value>,
    expected: &[ValidEntityRequest],
    owner_config_baselines: &HashMap<crate::vcp_modules::topic_types::OwnerKey, String>,
) -> Result<Vec<DecodedEntityResult>, String> {
    let expected_set = expected
        .iter()
        .map(|request| request.identity.clone())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut decoded = Vec::with_capacity(values.len());
    for value in values {
        let item = decode_entity_result(value, &expected_set, &mut seen, owner_config_baselines)?;
        decoded.push(item);
    }
    if seen != expected_set {
        let mut missing = expected_set
            .difference(&seen)
            .map(EntityIdentity::label)
            .collect::<Vec<_>>();
        missing.sort();
        return Err(format!(
            "Entity pull response is missing results {missing:?}"
        ));
    }
    Ok(decoded)
}

fn decode_entity_result(
    value: Value,
    expected: &HashSet<EntityIdentity>,
    seen: &mut HashSet<EntityIdentity>,
    owner_config_baselines: &HashMap<crate::vcp_modules::topic_types::OwnerKey, String>,
) -> Result<DecodedEntityResult, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Entity pull result must be an object".to_string())?;
    let identity = parse_result_identity(object)?;
    if !expected.contains(&identity) {
        return Err(format!(
            "Entity pull returned unexpected result {}",
            identity.label()
        ));
    }
    if !seen.insert(identity.clone()) {
        return Err(format!(
            "Entity pull returned duplicate result {}",
            identity.label()
        ));
    }
    let ok = object.get("ok").and_then(Value::as_bool).ok_or_else(|| {
        format!(
            "Entity pull result {} requires boolean ok",
            identity.label()
        )
    })?;
    let allowed = result_keys(&identity, ok);
    super::require_exact_keys(object, &allowed)?;
    if ok {
        decode_success_result(object, identity, owner_config_baselines)
    } else {
        decode_failure_result(object, identity)
    }
}

fn result_keys(identity: &EntityIdentity, ok: bool) -> Vec<&'static str> {
    let terminal = if ok { "data" } else { "error" };
    match identity {
        EntityIdentity::Owner { .. } => {
            vec!["entityType", "ownerType", "ownerId", "ok", terminal]
        }
        EntityIdentity::Topic(_) => vec![
            "entityType",
            "ownerType",
            "ownerId",
            "topicId",
            "ok",
            terminal,
        ],
    }
}

fn decode_success_result(
    object: &Map<String, Value>,
    identity: EntityIdentity,
    owner_config_baselines: &HashMap<crate::vcp_modules::topic_types::OwnerKey, String>,
) -> Result<DecodedEntityResult, String> {
    let data = object
        .get("data")
        .cloned()
        .ok_or_else(|| format!("Entity pull result {} requires data", identity.label()))?;
    let task = decode_entity_data(&identity, data, owner_config_baselines)?;
    Ok(DecodedEntityResult {
        identity,
        task: Some(task),
        error: None,
    })
}

fn decode_failure_result(
    object: &Map<String, Value>,
    identity: EntityIdentity,
) -> Result<DecodedEntityResult, String> {
    let error = object
        .get("error")
        .ok_or_else(|| format!("Entity pull result {} requires error", identity.label()))?;
    let error = parse_wire_sync_error(error)
        .map_err(|error| format!("Entity pull result {} error: {error}", identity.label()))?;
    let encoded = crate::vcp_modules::sync::sync_error::encode_wire_sync_error(&error)?;
    Ok(DecodedEntityResult {
        identity,
        task: None,
        error: Some(encoded),
    })
}

fn parse_result_identity(object: &Map<String, Value>) -> Result<EntityIdentity, String> {
    let entity_type = object
        .get("entityType")
        .and_then(Value::as_str)
        .ok_or_else(|| "Entity pull result requires entityType".to_string())?;
    let owner_type = parse_owner_type(object.get("ownerType"))?;
    let owner_id = non_empty_string(object.get("ownerId"), "ownerId")?;
    match entity_type {
        "owner" => Ok(EntityIdentity::Owner {
            owner_type,
            owner_id,
        }),
        "topic" => {
            let topic_id = non_empty_string(object.get("topicId"), "topicId")?;
            Ok(EntityIdentity::Topic(TopicKey::new(
                owner_type.as_str(),
                owner_id,
                topic_id,
            )))
        }
        other => Err(format!(
            "Entity pull result has unsupported entityType {other}"
        )),
    }
}

fn decode_entity_data(
    identity: &EntityIdentity,
    value: Value,
    owner_config_baselines: &HashMap<crate::vcp_modules::topic_types::OwnerKey, String>,
) -> Result<DbWriteTask, String> {
    match identity {
        EntityIdentity::Owner {
            owner_type: OwnerType::Agent,
            owner_id,
        } => {
            let dto = serde_json::from_value::<AgentSyncDTO>(value)
                .map_err(|error| format!("Invalid agent {owner_id}: {error}"))?;
            Ok(DbWriteTask::Agent {
                id: owner_id.clone(),
                dto,
                expected_config_hash: owner_config_baselines
                    .get(&crate::vcp_modules::topic_types::OwnerKey::new(
                        "agent", owner_id,
                    ))
                    .cloned(),
            })
        }
        EntityIdentity::Owner {
            owner_type: OwnerType::Group,
            owner_id,
        } => {
            let dto = serde_json::from_value::<GroupSyncDTO>(value)
                .map_err(|error| format!("Invalid group {owner_id}: {error}"))?;
            Ok(DbWriteTask::Group {
                id: owner_id.clone(),
                dto,
                expected_config_hash: owner_config_baselines
                    .get(&crate::vcp_modules::topic_types::OwnerKey::new(
                        "group", owner_id,
                    ))
                    .cloned(),
            })
        }
        EntityIdentity::Topic(key) if key.owner_type == "agent" => {
            let dto = serde_json::from_value::<AgentTopicSyncDTO>(value)
                .map_err(|error| format!("Invalid agent topic {}: {error}", key.topic_id))?;
            validate_topic_dto(key, &dto.id, &dto.owner_id)?;
            Ok(DbWriteTask::AgentTopic {
                topic_id: key.topic_id.clone(),
                dto,
            })
        }
        EntityIdentity::Topic(key) if key.owner_type == "group" => {
            let dto = serde_json::from_value::<GroupTopicSyncDTO>(value)
                .map_err(|error| format!("Invalid group topic {}: {error}", key.topic_id))?;
            validate_topic_dto(key, &dto.id, &dto.owner_id)?;
            Ok(DbWriteTask::GroupTopic {
                topic_id: key.topic_id.clone(),
                dto,
            })
        }
        _ => Err("Entity pull identity has unsupported owner type".to_string()),
    }
}

fn validate_topic_dto(key: &TopicKey, dto_id: &str, dto_owner_id: &str) -> Result<(), String> {
    if dto_id != key.topic_id || dto_owner_id != key.owner_id {
        return Err(format!(
            "Entity topic data identity conflicts with {}/{}/{}",
            key.owner_type, key.owner_id, key.topic_id
        ));
    }
    Ok(())
}
