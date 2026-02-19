use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{RerankCandidate, RerankResult, RerankerProvider};
use crate::InferError;

const COHERE_ENDPOINT: &str = "https://api.cohere.com/v2/rerank";
pub const COHERE_PROVIDER_NAME: &str = "cohere";
pub const COHERE_MODEL_NAME: &str = "rerank-v3.5";

#[derive(Debug, Clone)]
pub struct CohereRerankerProvider {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl CohereRerankerProvider {
    pub fn from_env(api_key_env: &str) -> Result<Self, InferError> {
        let api_key = std::env::var(api_key_env)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| InferError::MissingCohereApiKey(api_key_env.to_owned()))?;

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model: COHERE_MODEL_NAME.to_owned(),
        })
    }

    pub fn provider_name(&self) -> &'static str {
        COHERE_PROVIDER_NAME
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    async fn rerank_request(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError> {
        if candidates.is_empty() || top_n == 0 {
            return Ok(Vec::new());
        }

        let request = CohereRerankRequest {
            model: self.model.clone(),
            query: query.to_owned(),
            documents: candidates
                .iter()
                .map(|candidate| candidate.text.clone())
                .collect::<Vec<_>>(),
            top_n: top_n.min(candidates.len()),
        };

        let response = self
            .client
            .post(COHERE_ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json::<CohereRerankResponse>()
            .await?;

        let mut reranked = response
            .results
            .into_iter()
            .filter_map(|entry| {
                let candidate = candidates.get(entry.index)?;
                Some(RerankResult {
                    id: candidate.id.clone(),
                    score: entry.relevance_score,
                    original_rank: entry.index,
                })
            })
            .collect::<Vec<_>>();

        reranked.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.original_rank.cmp(&right.original_rank))
                .then_with(|| left.id.cmp(&right.id))
        });

        Ok(reranked)
    }
}

#[async_trait]
impl RerankerProvider for CohereRerankerProvider {
    async fn rerank(
        &self,
        query: &str,
        candidates: &[RerankCandidate],
        top_n: usize,
    ) -> Result<Vec<RerankResult>, InferError> {
        self.rerank_request(query, candidates, top_n).await
    }

    fn provider_name(&self) -> &str {
        COHERE_PROVIDER_NAME
    }
}

#[derive(Debug, Serialize)]
struct CohereRerankRequest {
    model: String,
    query: String,
    documents: Vec<String>,
    top_n: usize,
}

#[derive(Debug, Deserialize)]
struct CohereRerankResponse {
    #[serde(default)]
    results: Vec<CohereRerankEntry>,
}

#[derive(Debug, Deserialize)]
struct CohereRerankEntry {
    index: usize,
    relevance_score: f32,
}
