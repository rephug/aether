use aether_store::SymbolEmbeddingRecord;

/// Cosine distance between two SIR embeddings, or `None` when they aren't comparable:
/// vectors from different embedding providers or models live in different spaces even
/// when their widths match, so a model swap must not read as semantic drift.
pub(crate) fn cosine_distance_from_embeddings(
    previous: Option<&SymbolEmbeddingRecord>,
    current: Option<&SymbolEmbeddingRecord>,
) -> Option<f64> {
    let previous = previous?;
    let current = current?;
    if previous.provider.trim() != current.provider.trim()
        || previous.model.trim() != current.model.trim()
    {
        return None;
    }
    if previous.embedding.len() != current.embedding.len() || previous.embedding.is_empty() {
        return None;
    }

    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (left, right) in previous.embedding.iter().zip(current.embedding.iter()) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if !dot.is_finite()
        || !left_norm.is_finite()
        || !right_norm.is_finite()
        || left_norm <= f64::EPSILON
        || right_norm <= f64::EPSILON
    {
        return None;
    }

    let similarity = (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0);
    Some(1.0 - similarity)
}

#[cfg(test)]
mod tests {
    use aether_store::SymbolEmbeddingRecord;

    use super::cosine_distance_from_embeddings;

    fn embedding(provider: &str, model: &str, values: &[f32]) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: "sym".to_owned(),
            sir_hash: "hash".to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            embedding: values.to_vec(),
            updated_at: 1,
        }
    }

    #[test]
    fn same_space_vectors_have_a_distance() {
        let before = embedding("candle", "qwen3", &[1.0, 0.0]);
        let after = embedding("candle", "qwen3", &[0.0, 1.0]);
        let distance = cosine_distance_from_embeddings(Some(&before), Some(&after));
        assert!((distance.expect("comparable") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn vectors_from_another_provider_or_model_are_not_compared() {
        let before = embedding("candle", "qwen3", &[1.0, 0.0]);
        for after in [
            embedding("gemini", "qwen3", &[1.0, 0.0]),
            embedding("candle", "gemma", &[1.0, 0.0]),
        ] {
            assert_eq!(
                cosine_distance_from_embeddings(Some(&before), Some(&after)),
                None
            );
        }
    }

    #[test]
    fn non_finite_vectors_have_no_distance() {
        let before = embedding("candle", "qwen3", &[f32::NAN, 1.0]);
        let after = embedding("candle", "qwen3", &[1.0, 1.0]);
        assert_eq!(
            cosine_distance_from_embeddings(Some(&before), Some(&after)),
            None
        );
    }
}
