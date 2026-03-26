use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SirLevel {
    Leaf,
    File,
    Module,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SirAnnotation {
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_cases: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    /// Per-method dependency map for traits/structs. None for functions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method_dependencies: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileSir {
    pub intent: String,
    pub exports: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub symbol_count: usize,
    pub confidence: f32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SirError {
    #[error("intent is required")]
    EmptyIntent,
    #[error("confidence must be between 0.0 and 1.0")]
    InvalidConfidence,
    #[error("complexity must be one of: Low, Medium, High, Critical, Unknown")]
    InvalidComplexity,
}

pub fn validate_sir(sir: &SirAnnotation) -> Result<(), SirError> {
    if sir.intent.trim().is_empty() {
        return Err(SirError::EmptyIntent);
    }

    if !(0.0..=1.0).contains(&sir.confidence) {
        return Err(SirError::InvalidConfidence);
    }

    if sir.complexity.is_some() && normalize_complexity_label(sir.complexity.as_deref()).is_none() {
        return Err(SirError::InvalidComplexity);
    }

    Ok(())
}

pub fn canonicalize_sir_json(sir: &SirAnnotation) -> String {
    let behavior = normalize_optional_text(sir.behavior.as_deref());
    let mut inputs = sir.inputs.clone();
    let mut outputs = sir.outputs.clone();
    let mut side_effects = sir.side_effects.clone();
    let mut dependencies = sir.dependencies.clone();
    let mut error_modes = sir.error_modes.clone();
    let edge_cases = normalize_optional_text(sir.edge_cases.as_deref());
    let complexity = normalize_complexity_label(sir.complexity.as_deref());

    inputs.sort();
    outputs.sort();
    side_effects.sort();
    dependencies.sort();
    error_modes.sort();

    let mut canonical = BTreeMap::<&str, Value>::new();
    if let Some(behavior) = behavior {
        canonical.insert("behavior", Value::from(behavior));
    }
    if let Some(complexity) = complexity {
        canonical.insert("complexity", Value::from(complexity));
    }
    canonical.insert("confidence", Value::from(sir.confidence));
    canonical.insert("dependencies", Value::from(dependencies));
    if let Some(edge_cases) = edge_cases {
        canonical.insert("edge_cases", Value::from(edge_cases));
    }
    canonical.insert("error_modes", Value::from(error_modes));
    canonical.insert("inputs", Value::from(inputs));
    canonical.insert("intent", Value::from(sir.intent.clone()));
    if let Some(method_dependencies) = &sir.method_dependencies {
        let canonical_method_dependencies = method_dependencies
            .iter()
            .map(|(method_name, deps)| {
                let mut sorted_deps = deps.clone();
                sorted_deps.sort();
                (method_name.clone(), sorted_deps)
            })
            .collect::<BTreeMap<_, _>>();
        canonical.insert(
            "method_dependencies",
            serde_json::to_value(canonical_method_dependencies)
                .expect("canonical method dependency serialization cannot fail"),
        );
    }
    canonical.insert("outputs", Value::from(outputs));
    canonical.insert("side_effects", Value::from(side_effects));

    serde_json::to_string(&canonical).expect("canonical sir serialization cannot fail")
}

pub fn sir_hash(sir: &SirAnnotation) -> String {
    let canonical = canonicalize_sir_json(sir);
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

pub fn canonicalize_file_sir_json(sir: &FileSir) -> String {
    let mut exports = sir.exports.clone();
    let mut side_effects = sir.side_effects.clone();
    let mut dependencies = sir.dependencies.clone();
    let mut error_modes = sir.error_modes.clone();

    sort_and_dedupe(&mut exports);
    sort_and_dedupe(&mut side_effects);
    sort_and_dedupe(&mut dependencies);
    sort_and_dedupe(&mut error_modes);

    let mut canonical = BTreeMap::<&str, Value>::new();
    canonical.insert("confidence", Value::from(sir.confidence));
    canonical.insert("dependencies", Value::from(dependencies));
    canonical.insert("error_modes", Value::from(error_modes));
    canonical.insert("exports", Value::from(exports));
    canonical.insert("intent", Value::from(sir.intent.clone()));
    canonical.insert("side_effects", Value::from(side_effects));
    canonical.insert("symbol_count", Value::from(sir.symbol_count as u64));

    serde_json::to_string(&canonical).expect("canonical file sir serialization cannot fail")
}

pub fn file_sir_hash(sir: &FileSir) -> String {
    let canonical = canonicalize_file_sir_json(sir);
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

pub fn synthetic_file_sir_id(language: &str, path: &str) -> String {
    synthetic_sir_id("file", language, path)
}

pub fn synthetic_module_sir_id(language: &str, path: &str) -> String {
    synthetic_sir_id("module", language, path)
}

fn synthetic_sir_id(prefix: &str, language: &str, path: &str) -> String {
    let material = format!(
        "{}:{}:{}",
        prefix.trim(),
        language.trim().to_ascii_lowercase(),
        normalize_path(path.trim()),
    );
    blake3::hash(material.as_bytes()).to_hex().to_string()
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn normalize_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn normalize_complexity_label(value: Option<&str>) -> Option<String> {
    let normalized = normalize_optional_text(value)?;
    match normalized.to_ascii_lowercase().as_str() {
        "low" => Some("Low".to_owned()),
        "medium" => Some("Medium".to_owned()),
        "high" => Some("High".to_owned()),
        "critical" => Some("Critical".to_owned()),
        "unknown" => Some("Unknown".to_owned()),
        _ => None,
    }
}

fn sort_and_dedupe(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_sir() -> SirAnnotation {
        SirAnnotation {
            intent: "Summarize behavior".to_owned(),
            behavior: None,
            inputs: vec!["b".to_owned(), "a".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: vec!["network".to_owned(), "db".to_owned()],
            dependencies: vec!["serde".to_owned(), "tokio".to_owned()],
            error_modes: vec!["io_error".to_owned(), "timeout".to_owned()],
            confidence: 0.75,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        }
    }

    fn sample_method_dependencies() -> HashMap<String, Vec<String>> {
        HashMap::from([
            (
                "load".to_owned(),
                vec!["Record".to_owned(), "StoreError".to_owned()],
            ),
            ("delete".to_owned(), vec!["StoreError".to_owned()]),
        ])
    }

    #[test]
    fn validate_sir_rejects_empty_intent() {
        let mut sir = sample_sir();
        sir.intent = "   ".to_owned();

        let err = validate_sir(&sir).expect_err("expected error");
        assert_eq!(err, SirError::EmptyIntent);
    }

    #[test]
    fn validate_sir_rejects_out_of_range_confidence() {
        let mut sir = sample_sir();
        sir.confidence = 1.01;

        let err = validate_sir(&sir).expect_err("expected error");
        assert_eq!(err, SirError::InvalidConfidence);
    }

    #[test]
    fn canonicalization_is_stable_for_list_reordering() {
        let sir_a = sample_sir();
        let sir_b = SirAnnotation {
            intent: sir_a.intent.clone(),
            behavior: None,
            inputs: vec!["a".to_owned(), "b".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: vec!["db".to_owned(), "network".to_owned()],
            dependencies: vec!["tokio".to_owned(), "serde".to_owned()],
            error_modes: vec!["timeout".to_owned(), "io_error".to_owned()],
            confidence: 0.75,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        };

        let canonical_a = canonicalize_sir_json(&sir_a);
        let canonical_b = canonicalize_sir_json(&sir_b);

        assert_eq!(canonical_a, canonical_b);
        assert_eq!(
            canonical_a,
            "{\"confidence\":0.75,\"dependencies\":[\"serde\",\"tokio\"],\"error_modes\":[\"io_error\",\"timeout\"],\"inputs\":[\"a\",\"b\"],\"intent\":\"Summarize behavior\",\"outputs\":[\"result\"],\"side_effects\":[\"db\",\"network\"]}"
        );
    }

    #[test]
    fn hash_is_stable_for_canonical_equivalent_annotations() {
        let sir_a = sample_sir();
        let mut sir_b = sample_sir();
        sir_b.dependencies.reverse();
        sir_b.inputs.reverse();
        sir_b.side_effects.reverse();

        let hash_a = sir_hash(&sir_a);
        let hash_b = sir_hash(&sir_b);

        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn deserialize_without_method_dependencies_defaults_to_none() {
        let json = r#"{"intent":"Summarize behavior","inputs":["a"],"outputs":["result"],"side_effects":[],"dependencies":["serde"],"error_modes":[],"confidence":0.75}"#;

        let sir: SirAnnotation = serde_json::from_str(json).expect("sir should deserialize");
        assert_eq!(sir.method_dependencies, None);
        assert_eq!(sir.behavior, None);
        assert_eq!(sir.edge_cases, None);
        assert_eq!(sir.complexity, None);
    }

    #[test]
    fn sir_round_trips_with_method_dependencies() {
        let mut sir = sample_sir();
        sir.method_dependencies = Some(sample_method_dependencies());

        let encoded = serde_json::to_string(&sir).expect("sir should serialize");
        let decoded: SirAnnotation =
            serde_json::from_str(&encoded).expect("sir should deserialize");

        assert_eq!(decoded, sir);
    }

    #[test]
    fn canonicalization_is_stable_for_method_dependency_map_reordering() {
        let mut sir_a = sample_sir();
        sir_a.method_dependencies = Some(HashMap::from([
            (
                "save".to_owned(),
                vec!["StoreError".to_owned(), "Record".to_owned()],
            ),
            ("delete".to_owned(), vec!["StoreError".to_owned()]),
        ]));

        let mut sir_b = sample_sir();
        sir_b.method_dependencies = Some(HashMap::from([
            ("delete".to_owned(), vec!["StoreError".to_owned()]),
            (
                "save".to_owned(),
                vec!["Record".to_owned(), "StoreError".to_owned()],
            ),
        ]));

        assert_eq!(canonicalize_sir_json(&sir_a), canonicalize_sir_json(&sir_b));
        assert_eq!(
            canonicalize_sir_json(&sir_a),
            "{\"confidence\":0.75,\"dependencies\":[\"serde\",\"tokio\"],\"error_modes\":[\"io_error\",\"timeout\"],\"inputs\":[\"a\",\"b\"],\"intent\":\"Summarize behavior\",\"method_dependencies\":{\"delete\":[\"StoreError\"],\"save\":[\"Record\",\"StoreError\"]},\"outputs\":[\"result\"],\"side_effects\":[\"db\",\"network\"]}"
        );
    }

    #[test]
    fn serializing_without_method_dependencies_omits_the_field() {
        let sir = sample_sir();
        let encoded = serde_json::to_string(&sir).expect("sir should serialize");

        assert!(!encoded.contains("method_dependencies"));
    }

    #[test]
    fn validate_sir_rejects_invalid_complexity() {
        let mut sir = sample_sir();
        sir.complexity = Some("hard-ish".to_owned());

        let err = validate_sir(&sir).expect_err("expected invalid complexity");
        assert_eq!(err, SirError::InvalidComplexity);
    }

    #[test]
    fn complexity_normalizes_during_canonicalization() {
        let mut sir = sample_sir();
        sir.behavior = Some("  Performs work in stages.  ".to_owned());
        sir.edge_cases = Some("  Retries once on timeout. ".to_owned());
        sir.complexity = Some("critical".to_owned());

        assert_eq!(
            canonicalize_sir_json(&sir),
            "{\"behavior\":\"Performs work in stages.\",\"complexity\":\"Critical\",\"confidence\":0.75,\"dependencies\":[\"serde\",\"tokio\"],\"edge_cases\":\"Retries once on timeout.\",\"error_modes\":[\"io_error\",\"timeout\"],\"inputs\":[\"a\",\"b\"],\"intent\":\"Summarize behavior\",\"outputs\":[\"result\"],\"side_effects\":[\"db\",\"network\"]}"
        );
    }

    #[test]
    fn file_canonicalization_is_stable_for_list_reordering() {
        let file_a = FileSir {
            intent: "file summary".to_owned(),
            exports: vec!["beta".to_owned(), "alpha".to_owned()],
            side_effects: vec!["db".to_owned(), "network".to_owned()],
            dependencies: vec!["tokio".to_owned(), "serde".to_owned()],
            error_modes: vec!["timeout".to_owned(), "io".to_owned()],
            symbol_count: 2,
            confidence: 0.8,
        };
        let file_b = FileSir {
            intent: "file summary".to_owned(),
            exports: vec!["alpha".to_owned(), "beta".to_owned()],
            side_effects: vec!["network".to_owned(), "db".to_owned()],
            dependencies: vec!["serde".to_owned(), "tokio".to_owned()],
            error_modes: vec!["io".to_owned(), "timeout".to_owned()],
            symbol_count: 2,
            confidence: 0.8,
        };

        assert_eq!(
            canonicalize_file_sir_json(&file_a),
            canonicalize_file_sir_json(&file_b)
        );
        assert_eq!(file_sir_hash(&file_a), file_sir_hash(&file_b));
    }

    #[test]
    fn synthetic_file_and_module_ids_are_stable_and_normalized() {
        let file_a = synthetic_file_sir_id("rust", "src\\lib.rs");
        let file_b = synthetic_file_sir_id("rust", "src/lib.rs");
        assert_eq!(file_a, file_b);

        let module_a = synthetic_module_sir_id("typescript", "src\\features");
        let module_b = synthetic_module_sir_id("typescript", "src/features");
        assert_eq!(module_a, module_b);
        assert_ne!(file_a, module_a);
    }
}
