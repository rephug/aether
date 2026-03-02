use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aether_config::GraphBackend;
use aether_store::SurrealGraphStore;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CouplingQuery {
    pub min_score: Option<f64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CouplingSignals {
    pub temporal: f64,
    pub co_change: f64,
    pub structural: f64,
    pub static_signal: f64,
    pub semantic: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CouplingPair {
    pub file_a: String,
    pub file_b: String,
    pub coupling_score: f64,
    pub signals: CouplingSignals,
    pub coupling_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CouplingData {
    pub analysis_available: bool,
    pub pairs: Vec<CouplingPair>,
    pub total_pairs: usize,
    pub commits_scanned: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_mined_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CoChangeRow {
    file_a: String,
    file_b: String,
    fused_score: Option<f64>,
    git_coupling: Option<f64>,
    static_signal: Option<f64>,
    semantic_signal: Option<f64>,
    coupling_type: Option<String>,
}

pub(crate) async fn coupling_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<CouplingQuery>,
) -> impl IntoResponse {
    let min_score = query.min_score.unwrap_or(0.0).clamp(0.0, 1.0);
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        load_coupling_data(shared.as_ref(), min_score, limit).await
    })
    .await
    {
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

pub(crate) async fn load_coupling_data(
    shared: &SharedState,
    min_score: f64,
    limit: u32,
) -> Result<CouplingData, String> {
    let fallback = load_coupling_summary_sqlite(shared);

    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        let (last_commit_hash, last_mined_at, commits_scanned) = fallback;
        return Ok(CouplingData {
            analysis_available: last_mined_at.is_some() || commits_scanned > 0,
            pairs: Vec::new(),
            total_pairs: 0,
            commits_scanned,
            last_mined_at,
            last_commit_hash,
        });
    }

    let graph = SurrealGraphStore::open_readonly(shared.workspace.as_path())
        .await
        .map_err(|e| e.to_string())?;

    let query_text = r#"
        SELECT
            file_a,
            file_b,
            fused_score,
            git_coupling,
            static_signal,
            semantic_signal,
            coupling_type
        FROM co_change
        WHERE fused_score >= $min_score
        ORDER BY fused_score DESC
        LIMIT $limit;
    "#;

    let mut response = match graph
        .db()
        .query(query_text)
        .bind(("min_score", min_score as f32))
        .bind(("limit", limit as i64))
        .await
    {
        Ok(res) => res,
        Err(err) => {
            let message = err.to_string();
            if is_missing_surreal_relation(message.as_str()) {
                let (last_commit_hash, last_mined_at, commits_scanned) = fallback;
                return Ok(CouplingData {
                    analysis_available: last_mined_at.is_some() || commits_scanned > 0,
                    pairs: Vec::new(),
                    total_pairs: 0,
                    commits_scanned,
                    last_mined_at,
                    last_commit_hash,
                });
            }
            return Err(message);
        }
    };

    let rows: Vec<Value> = match response.take(0) {
        Ok(rows) => rows,
        Err(err) => return Err(err.to_string()),
    };

    let mut pairs = rows
        .into_iter()
        .filter_map(|row| serde_json::from_value::<CoChangeRow>(row).ok())
        .map(|row| {
            let coupling_score = row.fused_score.unwrap_or(0.0).clamp(0.0, 1.0);
            let temporal = row.git_coupling.unwrap_or(0.0).clamp(0.0, 1.0);
            let structural = row.static_signal.unwrap_or(0.0).clamp(0.0, 1.0);
            let semantic = row.semantic_signal.unwrap_or(0.0).clamp(0.0, 1.0);
            CouplingPair {
                file_a: support::normalized_display_path(row.file_a.as_str()),
                file_b: support::normalized_display_path(row.file_b.as_str()),
                coupling_score,
                signals: CouplingSignals {
                    temporal,
                    co_change: temporal,
                    structural,
                    static_signal: structural,
                    semantic,
                },
                coupling_type: row
                    .coupling_type
                    .and_then(|value| {
                        let trimmed = value.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_owned())
                        }
                    })
                    .unwrap_or_else(|| "temporal".to_owned()),
            }
        })
        .collect::<Vec<_>>();

    pairs.sort_by(|left, right| {
        right
            .coupling_score
            .partial_cmp(&left.coupling_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.file_a.cmp(&right.file_a))
            .then_with(|| left.file_b.cmp(&right.file_b))
    });

    let (last_commit_hash, last_mined_at, commits_scanned) = fallback;
    Ok(CouplingData {
        analysis_available: !pairs.is_empty() || last_mined_at.is_some() || commits_scanned > 0,
        total_pairs: pairs.len(),
        pairs,
        commits_scanned,
        last_mined_at,
        last_commit_hash,
    })
}

fn load_coupling_summary_sqlite(shared: &SharedState) -> (Option<String>, Option<i64>, i64) {
    let mut last_commit_hash = None;
    let mut last_mined_at = None;
    let mut commits_scanned = 0;

    if let Ok(Some(conn)) = support::open_meta_sqlite_ro(shared.workspace.as_path()) {
        let query = conn.query_row(
            "SELECT last_commit_hash, last_mined_at, commits_scanned FROM coupling_mining_state ORDER BY id ASC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, i64>(2)?.max(0),
                ))
            },
        );
        if let Ok((commit, mined_at, scanned)) = query {
            last_commit_hash = commit;
            last_mined_at = mined_at;
            commits_scanned = scanned;
        }
    }

    (last_commit_hash, last_mined_at, commits_scanned)
}

fn is_missing_surreal_relation(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("co_change")
        && (lower.contains("table")
            || lower.contains("relation")
            || lower.contains("not found")
            || lower.contains("does not exist"))
}
