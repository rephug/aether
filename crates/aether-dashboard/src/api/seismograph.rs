use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::support::{self, DashboardState};

// ─── Timeline ────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TimelineQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct MetricPoint {
    batch_timestamp: i64,
    codebase_shift: f64,
    semantic_velocity: f64,
    symbols_regenerated: i64,
    symbols_above_noise: i64,
}

#[derive(Debug, Serialize)]
struct CascadePoint {
    epicenter_symbol_id: String,
    total_hops: i64,
    max_delta_sem: f64,
    detected_at: i64,
    chain: Value,
}

#[derive(Debug, Serialize)]
struct TimelineData {
    velocity_series: Vec<MetricPoint>,
    cascades: Vec<CascadePoint>,
}

pub(crate) async fn seismograph_timeline_handler(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<TimelineQuery>,
) -> impl IntoResponse {
    let store = state.shared.store.clone();
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    match support::run_blocking_with_timeout(move || load_timeline(&store, limit)).await {
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

fn load_timeline(store: &aether_store::SqliteStore, limit: usize) -> Result<TimelineData, String> {
    let metrics = store.list_seismograph_metrics(limit).unwrap_or_default();

    let velocity_series: Vec<MetricPoint> = metrics
        .into_iter()
        .map(|m| MetricPoint {
            batch_timestamp: m.batch_timestamp,
            codebase_shift: m.codebase_shift,
            semantic_velocity: m.semantic_velocity,
            symbols_regenerated: m.symbols_regenerated,
            symbols_above_noise: m.symbols_above_noise,
        })
        .collect();

    let raw_cascades = store.list_cascades(limit).unwrap_or_default();

    let cascades: Vec<CascadePoint> = raw_cascades
        .into_iter()
        .map(|c| {
            let chain =
                serde_json::from_str::<Value>(&c.chain_json).unwrap_or(Value::Array(Vec::new()));
            CascadePoint {
                epicenter_symbol_id: c.epicenter_symbol_id,
                total_hops: c.total_hops,
                max_delta_sem: c.max_delta_sem,
                detected_at: c.detected_at,
                chain,
            }
        })
        .collect();

    Ok(TimelineData {
        velocity_series,
        cascades,
    })
}

// ─── Tectonic Plates ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct CommunityPoint {
    community_id: String,
    stability: f64,
    symbol_count: i64,
    breach_count: i64,
}

#[derive(Debug, Serialize)]
struct PlatesData {
    communities: Vec<CommunityPoint>,
}

pub(crate) async fn seismograph_plates_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let store = state.shared.store.clone();

    match support::run_blocking_with_timeout(move || load_plates(&store)).await {
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

fn load_plates(store: &aether_store::SqliteStore) -> Result<PlatesData, String> {
    let records = store.latest_community_stability().unwrap_or_default();

    let communities: Vec<CommunityPoint> = records
        .into_iter()
        .map(|r| CommunityPoint {
            community_id: r.community_id,
            stability: r.stability,
            symbol_count: r.symbol_count,
            breach_count: r.breach_count,
        })
        .collect();

    Ok(PlatesData { communities })
}

// ─── Velocity Gauge ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GaugeData {
    current_velocity: f64,
    previous_velocity: f64,
    trend: String,
    codebase_shift: f64,
    symbols_regenerated: i64,
    last_batch: i64,
}

pub(crate) async fn seismograph_gauge_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let store = state.shared.store.clone();

    match support::run_blocking_with_timeout(move || load_gauge(&store)).await {
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

fn load_gauge(store: &aether_store::SqliteStore) -> Result<GaugeData, String> {
    let metrics = store.list_seismograph_metrics(2).unwrap_or_default();

    if metrics.is_empty() {
        return Ok(GaugeData {
            current_velocity: 0.0,
            previous_velocity: 0.0,
            trend: "stable".to_owned(),
            codebase_shift: 0.0,
            symbols_regenerated: 0,
            last_batch: 0,
        });
    }

    let current = &metrics[0];
    let previous = metrics.get(1).unwrap_or(current);

    let trend = if current.semantic_velocity > previous.semantic_velocity * 1.1 {
        "accelerating"
    } else if current.semantic_velocity < previous.semantic_velocity * 0.9 {
        "decelerating"
    } else {
        "stable"
    };

    Ok(GaugeData {
        current_velocity: current.semantic_velocity,
        previous_velocity: previous.semantic_velocity,
        trend: trend.to_owned(),
        codebase_shift: current.codebase_shift,
        symbols_regenerated: current.symbols_regenerated,
        last_batch: current.batch_timestamp,
    })
}
