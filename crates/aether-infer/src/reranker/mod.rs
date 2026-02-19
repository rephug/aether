use std::collections::HashMap;

use async_trait::async_trait;

use crate::InferError;

pub mod candle;
pub mod cohere;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankCandidate {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RerankResult {
    pub id: String,
    pub score: f32,
    pub original_rank: usize,
}

#[async_trait]
pub trait RerankerProvider: Send + Sync {
    async fn rerank(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError>;

    fn provider_name(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct MockRerankerProvider {
    scores_by_id: HashMap<String, f32>,
    fallback_score: f32,
}

impl MockRerankerProvider {
    pub fn new(scores_by_id: HashMap<String, f32>) -> Self {
        Self {
            scores_by_id,
            fallback_score: 0.0,
        }
    }

    pub fn with_fallback_score(mut self, fallback_score: f32) -> Self {
        self.fallback_score = fallback_score;
        self
    }
}

impl Default for MockRerankerProvider {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}

#[async_trait]
impl RerankerProvider for MockRerankerProvider {
    async fn rerank(
        &self,
        _query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError> {
        let limit = top_n.min(candidates.len());
        let mut results = candidates
            .iter()
            .enumerate()
            .map(|(original_rank, candidate)| RerankResult {
                id: candidate.id.clone(),
                score: self
                    .scores_by_id
                    .get(&candidate.id)
                    .copied()
                    .unwrap_or(self.fallback_score),
                original_rank,
            })
            .collect::<Vec<_>>();

        results.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.original_rank.cmp(&right.original_rank))
                .then_with(|| left.id.cmp(&right.id))
        });
        results.truncate(limit);

        Ok(results)
    }

    fn provider_name(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_reranker_reorders_by_score() {
        let provider = MockRerankerProvider::new(HashMap::from([
            ("c".to_owned(), 0.9),
            ("a".to_owned(), 0.8),
            ("b".to_owned(), 0.1),
        ]));

        let candidates = vec![
            RerankCandidate {
                id: "a".to_owned(),
                text: "alpha".to_owned(),
            },
            RerankCandidate {
                id: "b".to_owned(),
                text: "beta".to_owned(),
            },
            RerankCandidate {
                id: "c".to_owned(),
                text: "gamma".to_owned(),
            },
        ];

        let reranked = provider
            .rerank("query", &candidates, 3)
            .await
            .expect("mock rerank");

        assert_eq!(reranked.len(), 3);
        assert_eq!(reranked[0].id, "c");
        assert_eq!(reranked[0].original_rank, 2);
        assert_eq!(reranked[1].id, "a");
        assert_eq!(reranked[1].original_rank, 0);
        assert_eq!(reranked[2].id, "b");
        assert_eq!(reranked[2].original_rank, 1);
    }
}
