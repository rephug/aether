use aether_store::SymbolEmbeddingRecord;

pub(crate) fn cosine_distance_from_embeddings(
    previous: Option<&SymbolEmbeddingRecord>,
    current: Option<&SymbolEmbeddingRecord>,
) -> Option<f64> {
    let previous = previous?;
    let current = current?;
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
    if left_norm <= f64::EPSILON || right_norm <= f64::EPSILON {
        return None;
    }

    Some(1.0 - (dot / (left_norm.sqrt() * right_norm.sqrt())))
}
