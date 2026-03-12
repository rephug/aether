mod embedding;
mod http;
mod loaders;
mod providers;
mod reranker;
mod sir_parsing;
pub mod sir_prompt;
mod types;

pub use embedding::Qwen3LocalEmbeddingProvider;
pub use http::{
    fetch_ollama_tags, normalize_ollama_endpoint, ollama_generate_endpoint, ollama_pull_endpoint,
    ollama_tags_endpoint, pull_ollama_model_with_progress,
};
pub use loaders::{
    download_candle_embedding_model, download_candle_reranker_model,
    load_embedding_provider_from_config, load_inference_provider_from_config,
    load_provider_from_env_or_mock, load_reranker_provider_from_config, summarize_text_with_config,
};
pub use providers::{GeminiProvider, OpenAiCompatProvider, Qwen3LocalProvider, TieredProvider};
pub use reranker::{MockRerankerProvider, RerankCandidate, RerankResult, RerankerProvider};
pub use types::{
    EmbeddingProvider, EmbeddingProviderOverrides, EmbeddingPurpose, GEMINI_API_KEY_ENV,
    InferError, InferSirResult, InferenceProvider, LoadedEmbeddingProvider, LoadedProvider,
    LoadedRerankerProvider, OllamaPullProgress, ProviderOverrides, RerankerProviderOverrides,
    SirContext,
};
