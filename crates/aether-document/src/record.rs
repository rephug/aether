use std::collections::BTreeMap;

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{DocumentError, Result};

pub trait SemanticRecord: Send + Sync {
    fn schema_name(&self) -> &str;
    fn schema_version(&self) -> &str;
    fn unit_id(&self) -> &str;
    fn as_json(&self) -> &Value;
    fn content_hash(&self) -> &str;
    fn embedding_text(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenericRecord {
    pub record_id: String,
    pub unit_id: String,
    pub domain: String,
    pub schema_name: String,
    pub schema_version: String,
    pub record_json: Value,
    pub content_hash: String,
    pub embedding_text: String,
}

impl GenericRecord {
    pub fn new(
        unit_id: impl Into<String>,
        domain: impl Into<String>,
        schema_name: impl Into<String>,
        schema_version: impl Into<String>,
        record_json: Value,
        embedding_text: impl Into<String>,
    ) -> Result<Self> {
        if !record_json.is_object() {
            return Err(DocumentError::InvalidRecordJson(
                "record_json must be a JSON object".to_owned(),
            ));
        }

        let unit_id = unit_id.into();
        let domain = domain.into();
        let schema_name = schema_name.into();
        let schema_version = schema_version.into();
        let embedding_text = embedding_text.into();

        let content_hash = canonical_content_hash(&record_json)?;
        let record_id = stable_record_id(unit_id.as_str(), schema_version.as_str(), content_hash.as_str());

        Ok(Self {
            record_id,
            unit_id,
            domain,
            schema_name,
            schema_version,
            record_json,
            content_hash,
            embedding_text,
        })
    }
}

impl SemanticRecord for GenericRecord {
    fn schema_name(&self) -> &str {
        self.schema_name.as_str()
    }

    fn schema_version(&self) -> &str {
        self.schema_version.as_str()
    }

    fn unit_id(&self) -> &str {
        self.unit_id.as_str()
    }

    fn as_json(&self) -> &Value {
        &self.record_json
    }

    fn content_hash(&self) -> &str {
        self.content_hash.as_str()
    }

    fn embedding_text(&self) -> &str {
        self.embedding_text.as_str()
    }
}

fn stable_record_id(unit_id: &str, schema_version: &str, content_hash: &str) -> String {
    let mut hasher = Hasher::new();
    hasher.update(unit_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(schema_version.as_bytes());
    hasher.update(b"\n");
    hasher.update(content_hash.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn canonical_content_hash(value: &Value) -> Result<String> {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Bool(v) => Value::Bool(*v),
        Value::Number(v) => Value::Number(v.clone()),
        Value::String(v) => Value::String(v.clone()),
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, item)| (key.clone(), canonicalize_json(item)))
                .collect::<BTreeMap<_, _>>();
            let mut canonical_map = Map::new();
            for (key, item) in sorted {
                canonical_map.insert(key, item);
            }
            Value::Object(canonical_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn generic_record_new_generates_deterministic_record_id() {
        let first = GenericRecord::new(
            "unit-1",
            "docs",
            "entity",
            "v1",
            json!({"name":"AETHER","type":"project"}),
            "AETHER project",
        )
        .expect("first record");
        let second = GenericRecord::new(
            "unit-1",
            "docs",
            "entity",
            "v1",
            json!({"type":"project","name":"AETHER"}),
            "AETHER project",
        )
        .expect("second record");

        assert_eq!(first.content_hash, second.content_hash);
        assert_eq!(first.record_id, second.record_id);
    }

    #[test]
    fn generic_record_hash_canonicalization_sorts_nested_keys_and_preserves_array_order() {
        let first = GenericRecord::new(
            "unit-1",
            "docs",
            "entity",
            "v1",
            json!({
                "z": {"b": 2, "a": 1},
                "arr": [{"y": 2, "x": 1}, {"b": 4, "a": 3}]
            }),
            "text",
        )
        .expect("first");
        let reordered_keys = GenericRecord::new(
            "unit-1",
            "docs",
            "entity",
            "v1",
            json!({
                "arr": [{"x": 1, "y": 2}, {"a": 3, "b": 4}],
                "z": {"a": 1, "b": 2}
            }),
            "text",
        )
        .expect("reordered");
        let reordered_array = GenericRecord::new(
            "unit-1",
            "docs",
            "entity",
            "v1",
            json!({
                "arr": [{"a": 3, "b": 4}, {"x": 1, "y": 2}],
                "z": {"a": 1, "b": 2}
            }),
            "text",
        )
        .expect("array order changed");

        assert_eq!(first.content_hash, reordered_keys.content_hash);
        assert_ne!(first.content_hash, reordered_array.content_hash);
    }

    #[test]
    fn generic_record_new_rejects_non_object_json() {
        let err = GenericRecord::new("unit-1", "docs", "entity", "v1", json!(["not", "object"]), "text")
            .expect_err("array should be rejected");
        match err {
            DocumentError::InvalidRecordJson(message) => {
                assert!(message.contains("JSON object"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
