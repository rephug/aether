use thiserror::Error;

use aether_sir::SirError;
use aether_store::StoreError;

#[derive(Debug, Error)]
pub enum AetherMcpError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("inference error: {0}")]
    Infer(#[from] aether_infer::InferError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("memory error: {0}")]
    Memory(#[from] aether_memory::MemoryError),
    #[error("analysis error: {0}")]
    Analysis(#[from] aether_analysis::AnalysisError),
    #[error("sir validation error: {0}")]
    Sir(#[from] SirError),
    #[error("read-only mode: {0}")]
    ReadOnly(String),
    #[error("{0}")]
    Message(String),
}
