use std::cmp::Ordering;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::common;
use crate::api::coupling;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct XrayQuery {
    pub window: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct XrayMetric {
    pub value: Option<serde_json::Value>,
    pub trend: Option<f64>,
    pub sparkline: Vec<f64>,
    pub not_computed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct XrayMetrics {
    pub sir_coverage: XrayMetric,
    pub orphan_count: XrayMetric,
    pub avg_drift: XrayMetric,
    pub graph_connectivity: XrayMetric,
    pub high_coupling_pairs: XrayMetric,
    pub sir_coverage_pct: XrayMetric,
    pub index_freshness_secs: XrayMetric,
    pub risk_grade: XrayMetric,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct XrayHotspot {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub risk_score: f64,
    pub pagerank: f64,
    pub drift_score: f64,
    pub test_count: i64,
    pub has_sir: bool,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct XrayData {
    pub metrics: XrayMetrics,
    pub hotspots: Vec<XrayHotspot>,
}

pub(crate) async fn xray_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<XrayQuery>,
) -> impl IntoResponse {
    match load_xray_data(state.shared.as_ref(), query.window.as_deref()).await {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => support::json_internal_error(err),
    }
}

pub(crate) async fn load_xray_data(
    shared: &crate::state::SharedState,
    window: Option<&str>,
) -> Result<XrayData, String> {
    let _cutoff = common::cutoff_millis_for_days(common::parse_window_days(window));

    let symbols = common::load_symbols(shared)?;
    if symbols.is_empty() {
        return Ok(XrayData {
            metrics: XrayMetrics {
                sir_coverage: metric_unavailable(),
                orphan_count: metric_unavailable(),
                avg_drift: metric_unavailable(),
                graph_connectivity: metric_unavailable(),
                high_coupling_pairs: metric_unavailable(),
                sir_coverage_pct: metric_unavailable(),
                index_freshness_secs: metric_unavailable(),
                risk_grade: metric_unavailable(),
            },
            hotspots: Vec::new(),
        });
    }

    let edges = common::load_dependency_algo_edges(shared)?;
    let pagerank = common::pagerank_map(shared, &edges).await;
    let drift_by_symbol = common::latest_drift_score_by_symbol(shared)?;
    let tests_by_symbol = common::test_count_by_symbol(shared)?;
    let sir_symbols = common::symbols_with_sir(shared)?;

    let max_pagerank = pagerank.values().copied().fold(0.0f64, f64::max);

    let mut hotspots = Vec::<XrayHotspot>::new();
    let mut risk_total = 0.0f64;
    for symbol in &symbols {
        let symbol_id = symbol.id.as_str();
        let pr = pagerank.get(symbol_id).copied().unwrap_or(0.0);
        let drift = drift_by_symbol.get(symbol_id).copied().unwrap_or(0.0);
        let test_count = tests_by_symbol.get(symbol_id).copied().unwrap_or(0);
        let has_sir = sir_symbols.contains(symbol_id);

        let score = common::risk_score(pr, drift, has_sir, test_count, max_pagerank);
        risk_total += score;

        let mut factors = Vec::new();
        if pr > 0.0 && max_pagerank > 0.0 && (pr / max_pagerank) >= 0.7 {
            factors.push("high_pagerank".to_owned());
        }
        if drift >= 0.3 {
            factors.push("semantic_drift".to_owned());
        }
        if !has_sir {
            factors.push("missing_sir".to_owned());
        }
        if test_count <= 0 {
            factors.push("no_tests".to_owned());
        }
        if factors.is_empty() {
            factors.push("stable".to_owned());
        }

        hotspots.push(XrayHotspot {
            symbol_id: symbol.id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            file_path: support::normalized_display_path(symbol.file_path.as_str()),
            risk_score: score,
            pagerank: pr,
            drift_score: drift,
            test_count,
            has_sir,
            risk_factors: factors,
        });
    }

    hotspots.sort_by(|left, right| {
        right
            .risk_score
            .partial_cmp(&left.risk_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                right
                    .pagerank
                    .partial_cmp(&left.pagerank)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });
    hotspots.truncate(10);

    let total_symbols = symbols.len() as i64;
    let sir_count = symbols
        .iter()
        .filter(|symbol| sir_symbols.contains(symbol.id.as_str()))
        .count() as i64;
    let sir_coverage_pct = if total_symbols > 0 {
        (sir_count as f64 / total_symbols as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let components = common::connected_components_vec(shared, &edges).await;
    let largest_component = components.first().map_or(0usize, Vec::len);
    let orphan_count = total_symbols.saturating_sub(largest_component as i64);
    let connectivity = if total_symbols > 0 {
        (largest_component as f64 / total_symbols as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let avg_drift = if symbols.is_empty() {
        0.0
    } else {
        let total = symbols
            .iter()
            .map(|symbol| {
                drift_by_symbol
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0.0)
            })
            .sum::<f64>();
        (total / symbols.len() as f64).clamp(0.0, 1.0)
    };

    let coupling_data = coupling::load_coupling_data(shared, 0.7, 200)
        .await
        .unwrap_or_else(|_| coupling::CouplingData {
            analysis_available: false,
            pairs: Vec::new(),
            total_pairs: 0,
            commits_scanned: 0,
            last_mined_at: None,
            last_commit_hash: None,
        });
    let high_coupling_pairs = coupling_data.pairs.len() as i64;

    let index_freshness_secs =
        support::compute_staleness(shared.workspace.as_path()).index_age_seconds;

    let mean_risk = if symbols.is_empty() {
        0.0
    } else {
        risk_total / symbols.len() as f64
    };
    let grade = common::risk_grade(mean_risk);

    let metrics = XrayMetrics {
        sir_coverage: metric_num(sir_count as f64, total_symbols > 0),
        orphan_count: metric_num(orphan_count as f64, total_symbols > 0),
        avg_drift: metric_num(avg_drift, !symbols.is_empty()),
        graph_connectivity: metric_num(connectivity, !symbols.is_empty()),
        high_coupling_pairs: XrayMetric {
            value: Some(json!(high_coupling_pairs)),
            trend: Some(0.0),
            sparkline: common::sparkline_placeholder(
                (high_coupling_pairs as f64 / 20.0).clamp(0.0, 1.0),
                30,
            ),
            not_computed: !coupling_data.analysis_available,
        },
        sir_coverage_pct: metric_num(sir_coverage_pct, total_symbols > 0),
        index_freshness_secs: XrayMetric {
            value: index_freshness_secs.map(|v| json!(v)),
            trend: Some(0.0),
            sparkline: common::sparkline_placeholder(
                index_freshness_secs
                    .map(|v| (1.0 - (v as f64 / 86_400.0)).clamp(0.0, 1.0))
                    .unwrap_or(0.5),
                30,
            ),
            not_computed: index_freshness_secs.is_none(),
        },
        risk_grade: XrayMetric {
            value: Some(json!(grade)),
            trend: Some(0.0),
            sparkline: common::sparkline_placeholder(mean_risk.clamp(0.0, 1.0), 30),
            not_computed: false,
        },
    };

    Ok(XrayData { metrics, hotspots })
}

fn metric_num(value: f64, available: bool) -> XrayMetric {
    XrayMetric {
        value: if available { Some(json!(value)) } else { None },
        trend: if available { Some(0.0) } else { None },
        sparkline: common::sparkline_placeholder(value.clamp(0.0, 1.0), 30),
        not_computed: !available,
    }
}

fn metric_unavailable() -> XrayMetric {
    XrayMetric {
        value: None,
        trend: None,
        sparkline: Vec::new(),
        not_computed: true,
    }
}
