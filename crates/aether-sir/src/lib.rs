use std::collections::BTreeMap;

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
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub confidence: f32,
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
}

pub fn validate_sir(sir: &SirAnnotation) -> Result<(), SirError> {
    if sir.intent.trim().is_empty() {
        return Err(SirError::EmptyIntent);
    }

    if !(0.0..=1.0).contains(&sir.confidence) {
        return Err(SirError::InvalidConfidence);
    }

    Ok(())
}

pub fn canonicalize_sir_json(sir: &SirAnnotation) -> String {
    let mut inputs = sir.inputs.clone();
    let mut outputs = sir.outputs.clone();
    let mut side_effects = sir.side_effects.clone();
    let mut dependencies = sir.dependencies.clone();
    let mut error_modes = sir.error_modes.clone();

    inputs.sort();
    outputs.sort();
    side_effects.sort();
    dependencies.sort();
    error_modes.sort();

    let mut canonical = BTreeMap::<&str, Value>::new();
    canonical.insert("confidence", Value::from(sir.confidence));
    canonical.insert("dependencies", Value::from(dependencies));
    canonical.insert("error_modes", Value::from(error_modes));
    canonical.insert("inputs", Value::from(inputs));
    canonical.insert("intent", Value::from(sir.intent.clone()));
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
            inputs: vec!["b".to_owned(), "a".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: vec!["network".to_owned(), "db".to_owned()],
            dependencies: vec!["serde".to_owned(), "tokio".to_owned()],
            error_modes: vec!["io_error".to_owned(), "timeout".to_owned()],
            confidence: 0.75,
        }
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
            inputs: vec!["a".to_owned(), "b".to_owned()],
            outputs: vec!["result".to_owned()],
            side_effects: vec!["db".to_owned(), "network".to_owned()],
            dependencies: vec!["tokio".to_owned(), "serde".to_owned()],
            error_modes: vec!["timeout".to_owned(), "io_error".to_owned()],
            confidence: 0.75,
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
