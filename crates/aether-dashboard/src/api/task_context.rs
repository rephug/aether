use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TaskContextQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TaskHistoryEntry {
    pub task_description: String,
    pub branch_name: Option<String>,
    pub symbol_count: i64,
    pub file_count: usize,
    pub budget_used: i64,
    pub budget_max: i64,
    pub budget_pct: f64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TaskContextData {
    pub has_history: bool,
    pub entries: Vec<TaskHistoryEntry>,
    pub total_entries: usize,
}

pub(crate) async fn task_context_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<TaskContextQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_task_context_data(shared.as_ref(), limit))
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

pub(crate) fn load_task_context_data(
    shared: &SharedState,
    limit: usize,
) -> Result<TaskContextData, String> {
    let history = shared
        .store
        .list_recent_task_history(limit)
        .map_err(|e| format!("failed to load task history: {e}"))?;

    let entries: Vec<TaskHistoryEntry> = history
        .into_iter()
        .map(|record| {
            let file_count = record
                .resolved_file_paths
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .count();
            let budget_pct = if record.budget_max > 0 {
                (record.budget_used as f64 / record.budget_max as f64 * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            TaskHistoryEntry {
                task_description: record.task_description,
                branch_name: record.branch_name,
                symbol_count: record.total_symbols,
                file_count,
                budget_used: record.budget_used,
                budget_max: record.budget_max,
                budget_pct,
                created_at: record.created_at,
            }
        })
        .collect();

    let total = entries.len();
    Ok(TaskContextData {
        has_history: !entries.is_empty(),
        entries,
        total_entries: total,
    })
}
