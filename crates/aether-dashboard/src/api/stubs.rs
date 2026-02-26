use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::support::{self, DashboardState};

#[derive(Debug, Serialize)]
struct GraphNode {
    id: String,
    label: String,
    kind: String,
    file: String,
    sir_exists: bool,
}

#[derive(Debug, Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    #[serde(rename = "type")]
    edge_type: String,
}

#[derive(Debug, Serialize)]
struct GraphData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    total_nodes: usize,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct DriftEntry {
    symbol_id: String,
    symbol_name: String,
    drift_type: String,
    drift_magnitude: f32,
    file_path: String,
    detected_at: i64,
}

#[derive(Debug, Serialize)]
struct DriftData {
    drift_entries: Vec<DriftEntry>,
    total_checked: i64,
    drifted_count: i64,
}

#[derive(Debug, Serialize)]
struct CouplingSignals {
    co_change: f32,
    static_signal: f32,
    semantic: f32,
}

#[derive(Debug, Serialize)]
struct CouplingPair {
    file_a: String,
    file_b: String,
    coupling_score: f32,
    signals: CouplingSignals,
}

#[derive(Debug, Serialize)]
struct CouplingData {
    pairs: Vec<CouplingPair>,
}

#[derive(Debug, Serialize)]
struct HealthHotspot {
    symbol_id: String,
    symbol_name: String,
    risk_score: f32,
    top_factor_description: String,
    pagerank: f32,
}

#[derive(Debug, Serialize)]
struct HealthData {
    analysis_available: bool,
    overall_score: Option<f32>,
    dimensions: Option<HealthDimensions>,
    hotspots: Vec<HealthHotspot>,
}

#[derive(Debug, Serialize)]
struct HealthDimensions {
    sir_coverage: f32,
    test_coverage: f32,
    coupling_health: f32,
    drift_health: f32,
}

pub(crate) async fn graph_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    support::api_json(
        state.shared.as_ref(),
        GraphData {
            nodes: Vec::new(),
            edges: Vec::new(),
            total_nodes: 0,
            truncated: false,
        },
    )
}

pub(crate) async fn drift_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    support::api_json(
        state.shared.as_ref(),
        DriftData {
            drift_entries: Vec::new(),
            total_checked: 0,
            drifted_count: 0,
        },
    )
}

pub(crate) async fn coupling_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    support::api_json(state.shared.as_ref(), CouplingData { pairs: Vec::new() })
}

pub(crate) async fn health_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    support::api_json(
        state.shared.as_ref(),
        HealthData {
            analysis_available: false,
            overall_score: None,
            dimensions: None,
            hotspots: Vec::new(),
        },
    )
}
