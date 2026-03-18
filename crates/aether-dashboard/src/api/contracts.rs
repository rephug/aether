use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::support::{self, DashboardState};

// ─── GET /api/v1/contracts ──────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ContractEntry {
    id: i64,
    symbol_id: String,
    symbol_name: String,
    clause_type: String,
    clause_text: String,
    active: bool,
    violation_streak: i64,
    status: String,
    created_at: i64,
    created_by: String,
}

#[derive(Debug, Serialize)]
struct ViolationEntry {
    id: i64,
    contract_id: i64,
    symbol_id: String,
    symbol_name: String,
    violation_type: String,
    confidence: Option<f64>,
    reason: Option<String>,
    detected_at: i64,
    dismissed: bool,
}

#[derive(Debug, Serialize)]
struct ContractSummary {
    total_contracts: usize,
    satisfied: usize,
    first_violation: usize,
    active_violation: usize,
    satisfaction_rate: f64,
}

#[derive(Debug, Serialize)]
struct ContractHealthData {
    contracts: Vec<ContractEntry>,
    summary: ContractSummary,
    recent_violations: Vec<ViolationEntry>,
}

const DEFAULT_STREAK_THRESHOLD: i64 = 2;

pub(crate) async fn contracts_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let store = state.shared.store.clone();
    let streak_threshold = state
        .shared
        .config
        .contracts
        .as_ref()
        .map(|c| i64::from(c.streak_threshold))
        .unwrap_or(DEFAULT_STREAK_THRESHOLD);

    match support::run_blocking_with_timeout(move || load_contracts(&store, streak_threshold)).await
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

fn load_contracts(
    store: &aether_store::SqliteStore,
    streak_threshold: i64,
) -> Result<ContractHealthData, String> {
    let raw_contracts = store
        .list_all_active_contracts()
        .map_err(|e| e.to_string())?;

    let mut satisfied = 0usize;
    let mut first_violation = 0usize;
    let mut active_violation = 0usize;

    let contracts: Vec<ContractEntry> = raw_contracts
        .iter()
        .map(|c| {
            let status = if c.violation_streak == 0 {
                satisfied += 1;
                "satisfied"
            } else if c.violation_streak < streak_threshold {
                first_violation += 1;
                "first_violation"
            } else {
                active_violation += 1;
                "active_violation"
            };

            let symbol_name = store
                .get_symbol_record(c.symbol_id.as_str())
                .ok()
                .flatten()
                .map(|s| s.qualified_name)
                .unwrap_or_else(|| c.symbol_id.clone());

            ContractEntry {
                id: c.id,
                symbol_id: c.symbol_id.clone(),
                symbol_name,
                clause_type: c.clause_type.clone(),
                clause_text: c.clause_text.clone(),
                active: c.active,
                violation_streak: c.violation_streak,
                status: status.to_owned(),
                created_at: c.created_at,
                created_by: c.created_by.clone(),
            }
        })
        .collect();

    let total = contracts.len();
    let satisfaction_rate = if total > 0 {
        satisfied as f64 / total as f64
    } else {
        1.0
    };

    let raw_violations = store
        .list_recent_violations(50)
        .map_err(|e| e.to_string())?;
    let recent_violations: Vec<ViolationEntry> = raw_violations
        .into_iter()
        .map(|v| {
            let symbol_name = store
                .get_symbol_record(v.symbol_id.as_str())
                .ok()
                .flatten()
                .map(|s| s.qualified_name)
                .unwrap_or_else(|| v.symbol_id.clone());

            ViolationEntry {
                id: v.id,
                contract_id: v.contract_id,
                symbol_id: v.symbol_id,
                symbol_name,
                violation_type: v.violation_type,
                confidence: v.confidence,
                reason: v.reason,
                detected_at: v.detected_at,
                dismissed: v.dismissed,
            }
        })
        .collect();

    Ok(ContractHealthData {
        contracts,
        summary: ContractSummary {
            total_contracts: total,
            satisfied,
            first_violation,
            active_violation,
            satisfaction_rate,
        },
        recent_violations,
    })
}

// ─── POST /api/v1/contracts/dismiss ─────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct DismissRequest {
    pub violation_id: i64,
    #[serde(default = "default_dismiss_reason")]
    pub reason: String,
}

fn default_dismiss_reason() -> String {
    "dismissed via dashboard".to_owned()
}

pub(crate) async fn dismiss_handler(
    State(state): State<Arc<DashboardState>>,
    Json(body): Json<DismissRequest>,
) -> impl IntoResponse {
    if state.shared.read_only {
        return support::json_internal_error(
            "Store is read-only. Dismiss via CLI: aetherd contract check --dismiss <id>".to_owned(),
        );
    }

    let store = state.shared.store.clone();
    let violation_id = body.violation_id;
    let reason = body.reason;

    match support::run_blocking_with_timeout(move || {
        store
            .dismiss_violation(violation_id, reason.as_str())
            .map(|()| serde_json::json!({ "success": true }))
            .map_err(|e| e.to_string())
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
