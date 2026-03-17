use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

/// Mirror of the batch-relevant fields from `ContinuousStatusSnapshot`
/// (defined in aetherd::continuous::monitor). We deserialise from the same
/// JSON file without depending on the aetherd crate.
#[derive(Debug, Clone, Default, Deserialize)]
struct StatusSnapshot {
    #[serde(default)]
    last_started_at: Option<i64>,
    #[serde(default)]
    last_completed_at: Option<i64>,
    #[serde(default)]
    written_requests: usize,
    #[serde(default)]
    skipped_requests: usize,
    #[serde(default)]
    chunk_count: usize,
    #[serde(default)]
    auto_submit: bool,
    #[serde(default)]
    submitted_chunks: usize,
    #[serde(default)]
    ingested_results: usize,
    #[serde(default)]
    fingerprint_rows: usize,
    #[serde(default)]
    requeue_pass: String,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BatchData {
    pub has_data: bool,
    pub last_run_at: Option<i64>,
    pub written_requests: usize,
    pub skipped_requests: usize,
    pub chunk_count: usize,
    pub auto_submit: bool,
    pub submitted_chunks: usize,
    pub ingested_results: usize,
    pub fingerprint_rows: usize,
    pub requeue_pass: String,
    pub last_error: Option<String>,
    pub batch_files: Vec<BatchFileEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BatchFileEntry {
    pub filename: String,
    pub size_bytes: u64,
}

pub(crate) async fn batch_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_batch_data(shared.as_ref())).await {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) fn load_batch_data(shared: &SharedState) -> Result<BatchData, String> {
    let aether_dir = aether_config::aether_dir(&shared.workspace);
    let status_path = aether_dir.join("continuous").join("status.json");

    let snapshot = if status_path.exists() {
        let raw = std::fs::read_to_string(&status_path)
            .map_err(|e| format!("failed to read status.json: {e}"))?;
        serde_json::from_str::<StatusSnapshot>(&raw)
            .map_err(|e| format!("failed to parse status.json: {e}"))?
    } else {
        StatusSnapshot::default()
    };

    let batch_dir = aether_dir.join("batch");
    let mut batch_files = Vec::new();
    if batch_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&batch_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                batch_files.push(BatchFileEntry {
                    filename,
                    size_bytes,
                });
            }
        }
    }
    batch_files.sort_by(|a, b| b.filename.cmp(&a.filename));

    let has_data = status_path.exists() || !batch_files.is_empty();

    Ok(BatchData {
        has_data,
        last_run_at: snapshot.last_completed_at.or(snapshot.last_started_at),
        written_requests: snapshot.written_requests,
        skipped_requests: snapshot.skipped_requests,
        chunk_count: snapshot.chunk_count,
        auto_submit: snapshot.auto_submit,
        submitted_chunks: snapshot.submitted_chunks,
        ingested_results: snapshot.ingested_results,
        fingerprint_rows: snapshot.fingerprint_rows,
        requeue_pass: snapshot.requeue_pass,
        last_error: snapshot.last_error,
        batch_files,
    })
}
