use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

mod dedup;
mod note;
mod ranking;
mod search;
mod unified_query;

pub use dedup::{compute_content_hash, compute_note_id, normalize_content_for_hash};
pub use note::{
    EntityRef, ListNotesRequest, NoteEmbeddingRequest, NoteSourceType, ProjectMemoryService,
    ProjectNote, RememberAction, RememberRequest, RememberResult, truncate_content_for_embedding,
};
pub use search::{RecallRequest, RecallResult, RecallScoredNote, SemanticQuery};
pub use unified_query::{
    AskInclude, AskQueryRequest, AskQueryResult, AskResultItem, AskResultKind,
};

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("store error: {0}")]
    Store(#[from] aether_store::StoreError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub(crate) fn current_unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
