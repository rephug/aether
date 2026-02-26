use thiserror::Error;

pub mod annotator;
pub mod edge;
pub mod embedding;
pub mod parser;
pub mod record;
pub mod registry;
pub mod temporal;
pub mod unit;
pub mod vector_backend;

#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("invalid document input: {0}")]
    InvalidInput(String),
    #[error("invalid semantic record json: {0}")]
    InvalidRecordJson(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("embedding provider error: {0}")]
    Infer(#[from] aether_infer::InferError),
    #[error("backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, DocumentError>;

pub use annotator::SemanticAnnotator;
pub use edge::{DocumentEdge, EdgeTypeRegistry};
pub use embedding::DocumentEmbedder;
pub use parser::DocumentParser;
pub use record::{GenericRecord, SemanticRecord};
pub use registry::VerticalRegistry;
pub use temporal::ChangeEvent;
pub use unit::{DocumentUnit, GenericUnit};
pub use vector_backend::{DocumentVectorBackend, DocumentVectorMatch};
