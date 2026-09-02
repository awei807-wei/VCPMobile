use serde_json::Value;

#[derive(Default)]
pub(crate) struct BoundedWarnings {
    pub(crate) count: usize,
    pub(crate) samples: Vec<String>,
}

impl BoundedWarnings {
    pub(crate) fn push(&mut self, message: String) {
        self.count += 1;
        if self.samples.len() < super::MAX_WARNING_SAMPLES {
            self.samples.push(message);
        }
    }
}

enum HashField {
    Missing,
    Valid(String),
    Invalid,
}

fn read_hash_field(object: &serde_json::Map<String, Value>, key: &str) -> HashField {
    match object.get(key) {
        None | Some(Value::Null) => HashField::Missing,
        Some(Value::String(hash)) => {
            let normalized = hash.to_ascii_lowercase();
            if crate::vcp_modules::infra::utils::is_valid_cas_hash(&normalized) {
                HashField::Valid(normalized)
            } else {
                HashField::Invalid
            }
        }
        Some(_) => HashField::Invalid,
    }
}

pub(crate) fn canonicalize_attachment(
    value: Value,
    message_id: &str,
    attachment_index: usize,
    warnings: &mut BoundedWarnings,
) -> Result<Option<Value>, String> {
    let mut object = match value {
        Value::Object(object) => object,
        _ => {
            return Err(format!(
                "Message {message_id} attachment {attachment_index} must be an object"
            ))
        }
    };

    let nested = match object.remove("_fileManagerData") {
        None | Some(Value::Null) => None,
        Some(Value::Object(nested)) => Some(nested),
        Some(_) => {
            warnings.push(format!(
                "message={message_id} attachment={attachment_index}: invalid _fileManagerData"
            ));
            return Ok(None);
        }
    };
    let top_hash = read_hash_field(&object, "hash");
    let nested_hash = nested
        .as_ref()
        .map(|nested| read_hash_field(nested, "hash"))
        .unwrap_or(HashField::Missing);
    let normalized_hash = match (top_hash, nested_hash) {
        (HashField::Valid(top), HashField::Valid(nested)) if top == nested => Some(top),
        (HashField::Valid(_), HashField::Valid(_)) => None,
        (HashField::Valid(hash), HashField::Missing | HashField::Invalid)
        | (HashField::Missing | HashField::Invalid, HashField::Valid(hash)) => Some(hash),
        (HashField::Missing | HashField::Invalid, HashField::Missing | HashField::Invalid) => None,
    };
    let Some(hash) = normalized_hash else {
        warnings.push(format!(
            "message={message_id} attachment={attachment_index}: missing, invalid, or conflicting SHA-256"
        ));
        return Ok(None);
    };

    move_nested_public_fields(&mut object, nested);
    object.insert("hash".to_string(), Value::String(hash));
    remove_local_attachment_fields(&mut object);
    Ok(Some(Value::Object(object)))
}

fn move_nested_public_fields(
    object: &mut serde_json::Map<String, Value>,
    nested: Option<serde_json::Map<String, Value>>,
) {
    let Some(mut nested) = nested else {
        return;
    };
    for nested_key in ["extractedText", "imageFrames", "createdAt"] {
        if !object.contains_key(nested_key) {
            if let Some(value) = nested.remove(nested_key) {
                object.insert(nested_key.to_string(), value);
            }
        }
    }
}

fn remove_local_attachment_fields(object: &mut serde_json::Map<String, Value>) {
    for key in [
        "_fileManagerData",
        "src",
        "resolvedSrc",
        "internalPath",
        "thumbnailPath",
        "path",
        "filePath",
        "status",
    ] {
        object.remove(key);
    }
}
