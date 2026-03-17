use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aether_config::GraphBackend;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_THRESHOLD: f64 = 0.3;
const MAX_MODULES: usize = 30;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CouplingMatrixQuery {
    pub granularity: Option<String>,
    pub threshold: Option<f64>,
}

#[derive(Debug, Serialize, Default, Clone)]
struct CouplingSignalBreakdown {
    temporal: f64,
    structural: f64,
    semantic: f64,
}

#[derive(Debug, Serialize)]
struct CouplingMatrixData {
    modules: Vec<String>,
    matrix: Vec<Vec<f64>>,
    signal_matrix: Vec<Vec<CouplingSignalBreakdown>>,
    total_pairs: usize,
}

#[derive(Debug, Deserialize)]
struct CoChangeRow {
    file_a: String,
    file_b: String,
    fused_score: Option<f64>,
    git_coupling: Option<f64>,
    static_signal: Option<f64>,
    semantic_signal: Option<f64>,
}

pub(crate) async fn coupling_matrix_handler(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<CouplingMatrixQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        load_coupling_matrix(shared.as_ref(), &params).await
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

async fn load_coupling_matrix(
    shared: &SharedState,
    params: &CouplingMatrixQuery,
) -> Result<CouplingMatrixData, String> {
    let threshold = params
        .threshold
        .unwrap_or(DEFAULT_THRESHOLD)
        .clamp(0.0, 1.0);
    let use_module_granularity = params
        .granularity
        .as_deref()
        .map(|g| g.eq_ignore_ascii_case("module"))
        .unwrap_or(true);

    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        return Ok(CouplingMatrixData {
            modules: Vec::new(),
            matrix: Vec::new(),
            signal_matrix: Vec::new(),
            total_pairs: 0,
        });
    }

    let graph = shared
        .surreal_graph_store()
        .await
        .map_err(|e| e.to_string())?;

    let query_text = r#"
        SELECT
            file_a,
            file_b,
            fused_score,
            git_coupling,
            static_signal,
            semantic_signal
        FROM co_change
        WHERE fused_score >= $min_score
        ORDER BY fused_score DESC
        LIMIT 500;
    "#;

    let mut response = match graph
        .db()
        .query(query_text)
        .bind(("min_score", threshold as f32))
        .await
    {
        Ok(res) => res,
        Err(err) => {
            let message = err.to_string();
            if message.to_ascii_lowercase().contains("co_change") {
                return Ok(CouplingMatrixData {
                    modules: Vec::new(),
                    matrix: Vec::new(),
                    signal_matrix: Vec::new(),
                    total_pairs: 0,
                });
            }
            return Err(message);
        }
    };

    let rows: Vec<Value> = response.take(0).map_err(|e| e.to_string())?;
    let pairs: Vec<CoChangeRow> = rows
        .into_iter()
        .filter_map(|row| serde_json::from_value(row).ok())
        .collect();
    let total_pairs = pairs.len();

    // Map file paths to module names (parent directory) if module granularity
    let to_key = |path: &str| -> String {
        if use_module_granularity {
            std::path::Path::new(path)
                .parent()
                .map(|pp| pp.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_owned())
        } else {
            support::normalized_display_path(path)
        }
    };

    // Collect all unique module names
    let mut module_set: HashMap<String, usize> = HashMap::new();
    for pair in &pairs {
        let key_a = to_key(&pair.file_a);
        let key_b = to_key(&pair.file_b);
        let next_idx = module_set.len();
        module_set.entry(key_a).or_insert(next_idx);
        let next_idx = module_set.len();
        module_set.entry(key_b).or_insert(next_idx);
    }

    // Sort modules alphabetically and reassign indices
    let mut modules_sorted: Vec<String> = module_set.keys().cloned().collect();
    modules_sorted.sort();
    modules_sorted.truncate(MAX_MODULES);

    let module_index: HashMap<String, usize> = modules_sorted
        .iter()
        .enumerate()
        .map(|(idx, name)| (name.clone(), idx))
        .collect();
    let n = modules_sorted.len();

    // Build accumulator matrices
    let mut score_sum = vec![vec![0.0f64; n]; n];
    let mut signal_sum = vec![vec![CouplingSignalBreakdown::default(); n]; n];
    let mut count = vec![vec![0usize; n]; n];

    for pair in &pairs {
        let key_a = to_key(&pair.file_a);
        let key_b = to_key(&pair.file_b);

        let idx_a = match module_index.get(&key_a) {
            Some(idx) => *idx,
            None => continue,
        };
        let idx_b = match module_index.get(&key_b) {
            Some(idx) => *idx,
            None => continue,
        };
        if idx_a == idx_b {
            continue; // skip self-coupling at module level
        }

        let fused = pair.fused_score.unwrap_or(0.0).clamp(0.0, 1.0);
        let temporal = pair.git_coupling.unwrap_or(0.0).clamp(0.0, 1.0);
        let structural = pair.static_signal.unwrap_or(0.0).clamp(0.0, 1.0);
        let semantic = pair.semantic_signal.unwrap_or(0.0).clamp(0.0, 1.0);

        // Symmetric: update both (i,j) and (j,i)
        for (a, b) in [(idx_a, idx_b), (idx_b, idx_a)] {
            score_sum[a][b] += fused;
            signal_sum[a][b].temporal += temporal;
            signal_sum[a][b].structural += structural;
            signal_sum[a][b].semantic += semantic;
            count[a][b] += 1;
        }
    }

    // Average the accumulated values
    let mut matrix = vec![vec![0.0f64; n]; n];
    let mut signal_matrix = vec![vec![CouplingSignalBreakdown::default(); n]; n];

    for i in 0..n {
        for j in 0..n {
            let c = count[i][j];
            if c > 0 {
                let cf = c as f64;
                matrix[i][j] = (score_sum[i][j] / cf).clamp(0.0, 1.0);
                signal_matrix[i][j] = CouplingSignalBreakdown {
                    temporal: (signal_sum[i][j].temporal / cf).clamp(0.0, 1.0),
                    structural: (signal_sum[i][j].structural / cf).clamp(0.0, 1.0),
                    semantic: (signal_sum[i][j].semantic / cf).clamp(0.0, 1.0),
                };
            }
        }
    }

    Ok(CouplingMatrixData {
        modules: modules_sorted,
        matrix,
        signal_matrix,
        total_pairs,
    })
}
