use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;
use serde_json::Value;

use aether_config::GraphBackend;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Serialize)]
struct ScorecardMetric {
    name: String,
    label: String,
    value: f64,
    unit: String,
    status: String,
    trend: Vec<f64>,
}

#[derive(Debug, Serialize)]
struct HealthScorecardData {
    metrics: Vec<ScorecardMetric>,
    overall_status: String,
}

pub(crate) async fn health_scorecard_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        load_health_scorecard(shared.as_ref()).await
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

async fn load_health_scorecard(shared: &SharedState) -> Result<HealthScorecardData, String> {
    let mut metrics = Vec::new();

    let (total_symbols, sir_count, avg_drift, orphan_count, index_freshness_hours) =
        load_sqlite_metrics(shared)?;

    // sir_coverage: SIR / total * 100
    let sir_coverage = if total_symbols > 0 {
        (sir_count as f64 / total_symbols as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    metrics.push(ScorecardMetric {
        name: "sir_coverage".to_owned(),
        label: "SIR Coverage".to_owned(),
        value: sir_coverage,
        unit: "%".to_owned(),
        status: if sir_coverage > 90.0 {
            "good"
        } else if sir_coverage > 70.0 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder(sir_coverage / 100.0, 8),
    });

    // stale_sir_pct: symbols without SIR / total * 100
    let stale_sir_pct = if total_symbols > 0 {
        ((total_symbols - sir_count).max(0) as f64 / total_symbols as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    metrics.push(ScorecardMetric {
        name: "stale_sir_pct".to_owned(),
        label: "Stale SIR".to_owned(),
        value: stale_sir_pct,
        unit: "%".to_owned(),
        status: if stale_sir_pct < 10.0 {
            "good"
        } else if stale_sir_pct < 30.0 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder(stale_sir_pct / 100.0, 8),
    });

    // orphan_count
    metrics.push(ScorecardMetric {
        name: "orphan_count".to_owned(),
        label: "Orphan Symbols".to_owned(),
        value: orphan_count as f64,
        unit: "count".to_owned(),
        status: if orphan_count < 5 {
            "good"
        } else if orphan_count < 20 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder((orphan_count as f64 / 50.0).min(1.0), 8),
    });

    // avg_drift_score
    metrics.push(ScorecardMetric {
        name: "avg_drift_score".to_owned(),
        label: "Avg Drift Score".to_owned(),
        value: avg_drift,
        unit: "score".to_owned(),
        status: if avg_drift < 0.15 {
            "good"
        } else if avg_drift < 0.30 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder(avg_drift, 8),
    });

    // graph_connectivity
    let graph_connectivity = compute_graph_connectivity(shared).await;
    metrics.push(ScorecardMetric {
        name: "graph_connectivity".to_owned(),
        label: "Graph Connectivity".to_owned(),
        value: graph_connectivity,
        unit: "ratio".to_owned(),
        status: if graph_connectivity > 0.9 {
            "good"
        } else if graph_connectivity > 0.7 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder(graph_connectivity, 8),
    });

    // high_coupling_pairs (> 0.7)
    let high_coupling_pairs = count_high_coupling_pairs(shared).await;
    metrics.push(ScorecardMetric {
        name: "high_coupling_pairs".to_owned(),
        label: "High Coupling Pairs".to_owned(),
        value: high_coupling_pairs as f64,
        unit: "count".to_owned(),
        status: if high_coupling_pairs < 3 {
            "good"
        } else if high_coupling_pairs < 8 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder(
            (high_coupling_pairs as f64 / 15.0).min(1.0),
            8,
        ),
    });

    // index_freshness_hours
    metrics.push(ScorecardMetric {
        name: "index_freshness_hours".to_owned(),
        label: "Index Freshness".to_owned(),
        value: index_freshness_hours,
        unit: "hours".to_owned(),
        status: if index_freshness_hours < 1.0 {
            "good"
        } else if index_freshness_hours < 24.0 {
            "warn"
        } else {
            "critical"
        }
        .to_owned(),
        trend: super::common::sparkline_placeholder((index_freshness_hours / 48.0).min(1.0), 8),
    });

    // Overall status: worst of all metrics
    let overall_status = if metrics.iter().any(|m| m.status == "critical") {
        "critical"
    } else if metrics.iter().any(|m| m.status == "warn") {
        "warn"
    } else {
        "good"
    }
    .to_owned();

    Ok(HealthScorecardData {
        metrics,
        overall_status,
    })
}

fn load_sqlite_metrics(shared: &SharedState) -> Result<(i64, i64, f64, i64, f64), String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok((0, 0, 0.0, 0, 0.0));
    };

    let total_symbols = support::count_table_rows(&conn, "symbols")
        .unwrap_or(0)
        .max(0);
    let sir_count = support::count_nonempty_sir(&conn).unwrap_or(0).max(0);

    // Average drift magnitude
    let avg_drift = match conn.query_row(
        "SELECT AVG(COALESCE(drift_magnitude, 0.0)) FROM drift_results",
        [],
        |row| row.get::<_, Option<f64>>(0),
    ) {
        Ok(Some(value)) => value.clamp(0.0, 1.0),
        Ok(None) => 0.0,
        Err(err) if support::is_missing_table(&err) => 0.0,
        Err(_) => 0.0,
    };

    // Orphan count: symbols with no edges (neither as source nor target)
    let orphan_count = match conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM symbols s
        WHERE NOT EXISTS (
            SELECT 1 FROM symbol_edges e
            WHERE e.source_id = s.id
        )
        AND NOT EXISTS (
            SELECT 1 FROM symbol_edges e2
            JOIN symbols t ON t.qualified_name = e2.target_qualified_name
            WHERE t.id = s.id
        )
        "#,
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(count) => count.max(0),
        Err(err) if support::is_missing_table(&err) => 0,
        Err(_) => 0,
    };

    // Index freshness
    let staleness = support::compute_staleness(shared.workspace.as_path());
    let index_freshness_hours = staleness
        .index_age_seconds
        .map(|secs| secs.max(0) as f64 / 3600.0)
        .unwrap_or(0.0);

    Ok((
        total_symbols,
        sir_count,
        avg_drift,
        orphan_count,
        index_freshness_hours,
    ))
}

async fn compute_graph_connectivity(shared: &SharedState) -> f64 {
    let symbols = match super::common::load_symbols(shared) {
        Ok(symbols) => symbols,
        Err(_) => return 0.0,
    };
    if symbols.is_empty() {
        return 0.0;
    }

    let edges = match super::common::load_dependency_algo_edges(shared) {
        Ok(edges) => edges,
        Err(_) => return 0.0,
    };

    let components = super::common::connected_components_vec(shared, &edges).await;
    let largest_component = components.iter().map(Vec::len).max().unwrap_or(0);
    (largest_component as f64 / symbols.len() as f64).clamp(0.0, 1.0)
}

async fn count_high_coupling_pairs(shared: &SharedState) -> i64 {
    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        return 0;
    }

    let graph = match shared.surreal_graph_store().await {
        Ok(graph) => graph,
        Err(_) => return 0,
    };

    let mut response = match graph
        .db()
        .query("SELECT VALUE count() FROM co_change WHERE fused_score >= 0.7 GROUP ALL;")
        .await
    {
        Ok(res) => res,
        Err(_) => return 0,
    };

    let rows: Vec<Value> = match response.take(0) {
        Ok(rows) => rows,
        Err(_) => return 0,
    };

    rows.first().and_then(Value::as_i64).unwrap_or(0).max(0)
}
