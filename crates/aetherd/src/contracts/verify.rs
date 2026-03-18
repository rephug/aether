/// Two-stage verification cascade for intent contracts.
///
/// Stage 1: Embedding cosine pre-filter (resolves ~90% of clauses).
/// Stage 2: LLM judge for ambiguous cases in the 0.50–0.88 range.
use aether_config::ContractsConfig;

use super::judge::{JudgeResult, judge_clause};

#[derive(Debug, Clone, PartialEq)]
pub enum ClauseStatus {
    Pass,
    Fail,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct ClauseResult {
    pub contract_id: i64,
    pub clause_text: String,
    pub clause_type: String,
    pub status: ClauseStatus,
    pub similarity: Option<f64>,
    pub judge_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    pub symbol_id: String,
    pub clauses_checked: usize,
    pub passed: usize,
    pub failed: usize,
    pub ambiguous: usize,
    pub clause_results: Vec<ClauseResult>,
}

pub struct ContractVerifier {
    pass_threshold: f64,
    fail_threshold: f64,
}

impl ContractVerifier {
    pub fn from_config(config: &ContractsConfig) -> Self {
        Self {
            pass_threshold: config.embedding_pass_threshold,
            fail_threshold: config.embedding_fail_threshold,
        }
    }

    /// Classify a single clause via embedding cosine similarity.
    pub fn classify_by_embedding(
        &self,
        clause_embedding: &[f32],
        sir_embedding: &[f32],
    ) -> (ClauseStatus, f64) {
        let similarity = cosine_similarity(clause_embedding, sir_embedding).unwrap_or(0.0);
        let status = if similarity > self.pass_threshold {
            ClauseStatus::Pass
        } else if similarity < self.fail_threshold {
            ClauseStatus::Fail
        } else {
            ClauseStatus::Ambiguous
        };
        (status, similarity)
    }

    /// Resolve an ambiguous clause using the LLM judge.
    pub fn resolve_ambiguous_with_judge(
        &self,
        clause_text: &str,
        clause_type: &str,
        sir_json: &str,
        workspace_root: &std::path::Path,
    ) -> ClauseStatus {
        match judge_clause(clause_text, clause_type, sir_json, workspace_root) {
            Ok(JudgeResult { violated: true, .. }) => ClauseStatus::Fail,
            Ok(JudgeResult {
                violated: false, ..
            }) => ClauseStatus::Pass,
            Err(err) => {
                tracing::debug!(error = %err, "LLM judge unavailable, leaving clause ambiguous");
                ClauseStatus::Ambiguous
            }
        }
    }
}

/// Compute cosine similarity between two embedding vectors.
/// Returns `None` if vectors are empty, different lengths, or zero-norm.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }

    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (left, right) in a.iter().zip(b.iter()) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        dot += left * right;
        norm_a += left * left;
        norm_b += right * right;
    }

    if norm_a <= f64::EPSILON || norm_b <= f64::EPSILON {
        return None;
    }

    let similarity = dot / (norm_a.sqrt() * norm_b.sqrt());
    Some(similarity.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_config::ContractsConfig;

    #[test]
    fn cosine_similarity_identical_vectors() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v).unwrap();
        assert!(
            (sim - 1.0).abs() < 1e-9,
            "identical vectors should be ~1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b).unwrap();
        assert!(
            sim.abs() < 1e-9,
            "orthogonal vectors should be ~0.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![-1.0_f32, 0.0];
        let sim = cosine_similarity(&a, &b).unwrap();
        assert!(
            (sim + 1.0).abs() < 1e-9,
            "opposite vectors should be ~-1.0, got {sim}"
        );
    }

    #[test]
    fn cosine_similarity_different_lengths() {
        let a = vec![1.0_f32, 2.0];
        let b = vec![1.0_f32, 2.0, 3.0];
        assert!(cosine_similarity(&a, &b).is_none());
    }

    #[test]
    fn cosine_similarity_empty_vectors() {
        let empty: Vec<f32> = vec![];
        assert!(cosine_similarity(&empty, &empty).is_none());
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![0.0_f32, 0.0, 0.0];
        let b = vec![1.0_f32, 2.0, 3.0];
        assert!(cosine_similarity(&a, &b).is_none());
    }

    #[test]
    fn high_similarity_passes() {
        let config = ContractsConfig::default();
        let verifier = ContractVerifier::from_config(&config);
        // Two nearly identical vectors
        let clause = vec![1.0_f32, 0.0, 0.0];
        let sir = vec![0.99_f32, 0.05, 0.0];
        let (status, sim) = verifier.classify_by_embedding(&clause, &sir);
        assert!(sim > 0.88, "similarity should be > 0.88, got {sim}");
        assert_eq!(status, ClauseStatus::Pass);
    }

    #[test]
    fn low_similarity_fails() {
        let config = ContractsConfig::default();
        let verifier = ContractVerifier::from_config(&config);
        // Nearly orthogonal vectors
        let clause = vec![1.0_f32, 0.0, 0.0];
        let sir = vec![0.1_f32, 0.95, 0.3];
        let (status, sim) = verifier.classify_by_embedding(&clause, &sir);
        assert!(sim < 0.50, "similarity should be < 0.50, got {sim}");
        assert_eq!(status, ClauseStatus::Fail);
    }

    #[test]
    fn mid_similarity_is_ambiguous() {
        let config = ContractsConfig::default();
        let verifier = ContractVerifier::from_config(&config);
        // Vectors with moderate similarity
        let clause = vec![1.0_f32, 0.0, 0.0];
        let sir = vec![0.7_f32, 0.7, 0.0];
        let (status, sim) = verifier.classify_by_embedding(&clause, &sir);
        assert!(
            sim >= 0.50 && sim <= 0.88,
            "similarity should be in ambiguous range, got {sim}"
        );
        assert_eq!(status, ClauseStatus::Ambiguous);
    }
}
