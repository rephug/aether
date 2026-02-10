use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

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
}
