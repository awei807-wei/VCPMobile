use crate::vcp_modules::sync_service::SyncCommand;
use crate::vcp_modules::sync_types::SyncDataType;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;

pub(crate) fn consume_manifest_response_type(
    payload: &Value,
    data_type: &SyncDataType,
    current_phase: u8,
    expected_manifest_types: &Mutex<HashSet<String>>,
) -> Result<bool, String> {
    let expected_wire_phase = wire_phase(current_phase);
    let msg_phase = match payload.get("phase") {
        Some(value) => value
            .as_u64()
            .and_then(|phase| u8::try_from(phase).ok())
            .ok_or_else(|| "SYNC_DIFF_RESULTS.phase must be an integer".to_string())?,
        None if is_exact_central_manifest_response(payload, data_type) => expected_wire_phase,
        None => return Err("SYNC_DIFF_RESULTS.phase is missing outside CDS mode".to_string()),
    };
    if msg_phase != expected_wire_phase {
        return Err(format!(
            "SYNC_DIFF_RESULTS phase mismatch: expected {expected_wire_phase}, got {msg_phase}"
        ));
    }
    let data_type_name = data_type.to_string();
    let mut remaining = expected_manifest_types
        .lock()
        .map_err(|_| "Expected manifest type set is poisoned".to_string())?;
    if !remaining.remove(&data_type_name) {
        return Err(format!(
            "SYNC_DIFF_RESULTS contains duplicate or unexpected dataType {data_type_name} for phase {current_phase}"
        ));
    }
    Ok(remaining.is_empty())
}

fn is_exact_central_manifest_response(payload: &Value, data_type: &SyncDataType) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };
    let expected_data_type = data_type.to_string();
    object.len() == 3
        && object.get("type").and_then(Value::as_str) == Some("SYNC_DIFF_RESULTS")
        && object.get("data").is_some_and(Value::is_array)
        && object.get("dataType").and_then(Value::as_str) == Some(expected_data_type.as_str())
}

fn wire_phase(current_phase: u8) -> u8 {
    match current_phase {
        1 | 2 => 1,
        3 => 2,
        _ => current_phase,
    }
}

pub(crate) fn next_manifest_command(current_phase: u8, attempt_id: u64) -> Option<SyncCommand> {
    match current_phase {
        1 => Some(SyncCommand::StartAvatarMetadata { attempt_id }),
        2 => Some(SyncCommand::StartTopicMetadata { attempt_id }),
        3 => Some(SyncCommand::StartTopicValidation { attempt_id }),
        _ => None,
    }
}
