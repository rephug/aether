pub mod candle;
pub mod gemini_native;
pub mod openai_compat;

/// Describes why an embedding is being generated.
/// Used by task-type-aware providers to optimize embeddings for retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingPurpose {
    /// Embedding stored content for indexing and retrieval.
    #[default]
    Document,
    /// Embedding a user query for semantic search.
    Query,
}
