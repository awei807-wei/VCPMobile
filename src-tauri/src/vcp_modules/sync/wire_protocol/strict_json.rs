//! Strict JSON decoding for wire frames.

use serde::de::{DeserializeSeed, Deserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Errors returned while decoding a strict JSON frame.
#[derive(Debug)]
pub enum StrictJsonError {
    /// The input is not one complete JSON value.
    Json(serde_json::Error),
}

impl fmt::Display for StrictJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StrictJsonError {}

impl From<serde_json::Error> for StrictJsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Parses exactly one JSON value and rejects duplicate object keys at every depth.
pub fn parse_strict_json(input: &str) -> Result<Value, StrictJsonError> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = deserializer.deserialize_any(StrictValueVisitor)?;
    deserializer.end()?;
    Ok(value)
}

/// Compatibility name used by the JavaScript protocol implementation.
pub fn parse_json_without_duplicate_keys(input: &str) -> Result<Value, StrictJsonError> {
    parse_strict_json(input)
}

struct StrictValueSeed;

impl<'de> DeserializeSeed<'de> for StrictValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: DeError,
    {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(A::Error::custom(format!(
                    "duplicate JSON object key {key:?}"
                )));
            }
            let value = map.next_value_seed(StrictValueSeed)?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_strict_json;

    #[test]
    fn accepts_nested_json_without_duplicate_keys() {
        let value = parse_strict_json(r#"{"outer":[{"inner":true},null]}"#)
            .expect("valid nested JSON should parse");
        assert_eq!(value["outer"][0]["inner"], true);
    }

    #[test]
    fn rejects_duplicate_keys_at_the_root() {
        let error = parse_strict_json(r#"{"type":"VERSION_ACK","type":"SYNC_ERROR"}"#)
            .expect_err("duplicate keys must fail closed");
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_duplicate_keys_inside_nested_objects() {
        let error = parse_strict_json(r#"{"error":{"code":"A","code":"B"}}"#)
            .expect_err("nested duplicate keys must fail closed");
        assert!(error.to_string().contains("duplicate"));
    }

    #[test]
    fn rejects_trailing_json_values() {
        let error = parse_strict_json(r#"{"type":"VERSION_ACK"} {"extra":true}"#)
            .expect_err("trailing JSON must fail closed");
        assert!(error.to_string().contains("trailing") || error.to_string().contains("eof"));
    }
}
