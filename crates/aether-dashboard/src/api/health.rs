use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aether_analysis::{HealthAnalyzer, HealthInclude, HealthRequest};
use aether_config::GraphBackend;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct HealthQuery {
    pub limit: Option<u32>,
    pub min_risk: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthDimensions {
    pub sir_coverage: f64,
    pub test_coverage: f64,
    pub graph_connectivity: f64,
    pub coupling_health: f64,
    pub drift_health: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthHotspot {
    pub symbol_id: String,
    pub symbol_name: String,
    pub risk_score: f64,
    pub top_factor_description: String,
    pub pagerank: f64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthCycle {
    pub cycle_id: u32,
    pub symbols: Vec<String>,
    pub edge_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthOrphan {
    pub subgraph_id: u32,
    pub symbols: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthData {
    pub analysis_available: bool,
    pub overall_score: Option<f64>,
    pub notes: Vec<String>,
    pub dimensions: HealthDimensions,
    pub hotspots: Vec<HealthHotspot>,
    pub cycles: Vec<HealthCycle>,
    pub orphans: Vec<HealthOrphan>,
}

pub(crate) async fn health_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<HealthQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        Ok(load_health_data(shared.as_ref(), &query).await)
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

pub(crate) async fn load_health_data(shared: &SharedState, query: &HealthQuery) -> HealthData {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let min_risk = query.min_risk.unwrap_or(0.0).clamp(0.0, 1.0);

    let mut notes = Vec::<String>::new();
    let mut hotspots = Vec::<HealthHotspot>::new();
    let mut cycles = Vec::<HealthCycle>::new();
    let mut orphans = Vec::<HealthOrphan>::new();
    let mut analysis_available = false;

    if shared.config.storage.graph_backend == GraphBackend::Surreal {
        let analyzer = match HealthAnalyzer::new(shared.workspace.as_path()) {
            Ok(analyzer) => analyzer,
            Err(err) => {
                push_note(&mut notes, format!("Health analyzer setup failed: {err}"));
                return health_data_with_dimensions(
                    shared,
                    analysis_available,
                    notes,
                    hotspots,
                    cycles,
                    orphans,
                )
                .await;
            }
        };

        let request = HealthRequest {
            include: vec![
                HealthInclude::CriticalSymbols,
                HealthInclude::Bottlenecks,
                HealthInclude::Cycles,
                HealthInclude::Orphans,
                HealthInclude::RiskHotspots,
            ],
            limit,
            min_risk,
        };

        match shared.surreal_graph_store().await {
            Ok(graph) => match analyzer.analyze_with_graph(&request, graph.as_ref()).await {
                Ok(report) => {
                    analysis_available = report.analysis.total_symbols > 0;
                    for note in &report.notes {
                        push_note(&mut notes, note);
                    }

                    let pagerank_by_symbol = report
                        .critical_symbols
                        .iter()
                        .map(|entry| (entry.symbol_id.clone(), entry.pagerank))
                        .collect::<HashMap<_, _>>();

                    hotspots = report
                        .risk_hotspots
                        .into_iter()
                        .map(|entry| {
                            let top_factor =
                                entry.risk_factors.first().cloned().unwrap_or_else(|| {
                                    "No risk factor details available".to_owned()
                                });
                            HealthHotspot {
                                symbol_id: entry.symbol_id.clone(),
                                symbol_name: support::symbol_name_from_qualified(
                                    entry.symbol_name.as_str(),
                                ),
                                risk_score: entry.risk_score.clamp(0.0, 1.0),
                                top_factor_description: top_factor,
                                pagerank: pagerank_by_symbol
                                    .get(entry.symbol_id.as_str())
                                    .copied()
                                    .unwrap_or(0.0)
                                    .clamp(0.0, 1.0),
                            }
                        })
                        .collect();

                    cycles = report
                        .cycles
                        .into_iter()
                        .map(|entry| HealthCycle {
                            cycle_id: entry.cycle_id,
                            symbols: entry
                                .symbols
                                .into_iter()
                                .map(|symbol| {
                                    support::symbol_name_from_qualified(symbol.name.as_str())
                                })
                                .collect(),
                            edge_count: entry.edge_count,
                            note: entry.note,
                        })
                        .collect();

                    orphans = report
                        .orphans
                        .into_iter()
                        .map(|entry| HealthOrphan {
                            subgraph_id: entry.subgraph_id,
                            symbols: entry
                                .symbols
                                .into_iter()
                                .map(|symbol| {
                                    support::symbol_name_from_qualified(symbol.name.as_str())
                                })
                                .collect(),
                            note: entry.note,
                        })
                        .collect();
                }
                Err(err) => {
                    push_note(&mut notes, format!("Health analysis failed: {err}"));
                }
            },
            Err(err) => {
                push_note(&mut notes, format!("Surreal graph unavailable: {err}"));
            }
        }
    } else {
        push_note(
            &mut notes,
            "Health analysis requires surreal graph backend for full metrics",
        );
    }

    health_data_with_dimensions(shared, analysis_available, notes, hotspots, cycles, orphans).await
}

async fn health_data_with_dimensions(
    shared: &SharedState,
    analysis_available: bool,
    mut notes: Vec<String>,
    hotspots: Vec<HealthHotspot>,
    cycles: Vec<HealthCycle>,
    orphans: Vec<HealthOrphan>,
) -> HealthData {
    let (sir_coverage, test_coverage) = compute_sir_test_dimensions(shared, &mut notes);
    let graph_connectivity = compute_graph_connectivity(shared, &mut notes).await;
    let coupling_health = compute_coupling_health(shared, &mut notes).await;
    let drift_health = compute_drift_health(shared, &mut notes);

    let dimensions = HealthDimensions {
        sir_coverage,
        test_coverage,
        graph_connectivity,
        coupling_health,
        drift_health,
    };

    let base_score = (dimensions.sir_coverage
        + dimensions.test_coverage
        + dimensions.graph_connectivity
        + dimensions.coupling_health
        + dimensions.drift_health)
        / 5.0;
    let overall_score = if analysis_available {
        Some(base_score.clamp(0.0, 1.0))
    } else {
        None
    };

    HealthData {
        analysis_available,
        overall_score,
        notes,
        dimensions,
        hotspots,
        cycles,
        orphans,
    }
}

fn compute_sir_test_dimensions(shared: &SharedState, notes: &mut Vec<String>) -> (f64, f64) {
    let Some(conn) = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .ok()
        .flatten()
    else {
        push_note(
            notes,
            "SQLite metadata unavailable while computing health dimensions",
        );
        return (0.0, 0.0);
    };

    let total_symbols = support::count_table_rows(&conn, "symbols")
        .unwrap_or(0)
        .max(0);
    if total_symbols == 0 {
        push_note(
            notes,
            "No symbols available for health dimension computation",
        );
        return (0.0, 0.0);
    }

    let sir_count = support::count_nonempty_sir(&conn)
        .unwrap_or(0)
        .clamp(0, total_symbols);
    let sir_coverage = (sir_count as f64 / total_symbols as f64).clamp(0.0, 1.0);

    let tested_symbols = match conn.query_row(
        "SELECT COUNT(DISTINCT symbol_id) FROM test_intents WHERE TRIM(COALESCE(symbol_id, '')) <> ''",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(count) => count.max(0),
        Err(err) if support::is_missing_table(&err) => {
            push_note(notes, "test_intents table missing; test coverage dimension set to 0");
            0
        }
        Err(err) => {
            push_note(notes, format!("Failed to read test coverage data: {err}"));
            0
        }
    };

    let test_coverage =
        (tested_symbols.min(total_symbols) as f64 / total_symbols as f64).clamp(0.0, 1.0);
    (sir_coverage, test_coverage)
}

async fn compute_coupling_health(shared: &SharedState, notes: &mut Vec<String>) -> f64 {
    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        return 0.0;
    }

    let graph = match shared.surreal_graph_store().await {
        Ok(graph) => graph,
        Err(err) => {
            push_note(notes, format!("Coupling dimension unavailable: {err}"));
            return 0.0;
        }
    };

    let mut response = match graph
        .db()
        .query("SELECT VALUE math::mean(fused_score) FROM co_change GROUP ALL;")
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let message = err.to_string();
            if message.to_ascii_lowercase().contains("co_change") {
                push_note(
                    notes,
                    "No co_change rows found; coupling health defaults to 1.0",
                );
                return 1.0;
            }
            push_note(notes, format!("Coupling dimension query failed: {message}"));
            return 0.0;
        }
    };

    let rows: Vec<Value> = match response.take(0) {
        Ok(rows) => rows,
        Err(err) => {
            push_note(notes, format!("Coupling dimension decode failed: {err}"));
            return 0.0;
        }
    };

    let average = rows.first().and_then(Value::as_f64);
    match average {
        Some(value) => (1.0 - value.clamp(0.0, 1.0)).clamp(0.0, 1.0),
        None => {
            push_note(
                notes,
                "No coupling values available; coupling health defaults to 1.0",
            );
            1.0
        }
    }
}

async fn compute_graph_connectivity(shared: &SharedState, notes: &mut Vec<String>) -> f64 {
    let symbols = match crate::api::common::load_symbols(shared) {
        Ok(symbols) => symbols,
        Err(err) => {
            push_note(
                notes,
                format!("Failed to load symbols for graph connectivity: {err}"),
            );
            return 0.0;
        }
    };
    if symbols.is_empty() {
        return 0.0;
    }

    let edges = match crate::api::common::load_dependency_algo_edges(shared) {
        Ok(edges) => edges,
        Err(err) => {
            push_note(
                notes,
                format!("Failed to load graph edges for connectivity: {err}"),
            );
            return 0.0;
        }
    };

    let components = crate::api::common::connected_components_vec(shared, &edges).await;
    let largest_component = components.iter().map(Vec::len).max().unwrap_or(0);
    (largest_component as f64 / symbols.len() as f64).clamp(0.0, 1.0)
}

fn compute_drift_health(shared: &SharedState, notes: &mut Vec<String>) -> f64 {
    let Some(conn) = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .ok()
        .flatten()
    else {
        push_note(notes, "SQLite metadata unavailable for drift dimension");
        return 0.0;
    };

    let (symbols_analyzed, drift_detected) = match conn.query_row(
        "SELECT symbols_analyzed, drift_detected FROM drift_analysis_state ORDER BY id ASC LIMIT 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?.max(0), row.get::<_, i64>(1)?.max(0))),
    ) {
        Ok(values) => values,
        Err(err) if support::is_missing_table(&err) => {
            push_note(
                notes,
                "drift_analysis_state table missing; drift health dimension set to 0",
            );
            return 0.0;
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            push_note(
                notes,
                "No drift analysis summary found; drift health dimension set to 0",
            );
            return 0.0;
        }
        Err(err) => {
            push_note(
                notes,
                format!("Failed to read drift analysis summary: {err}"),
            );
            return 0.0;
        }
    };

    if symbols_analyzed <= 0 {
        push_note(notes, "Drift analysis has not processed symbols yet");
        return 0.0;
    }

    let average_magnitude = match conn.query_row(
        "SELECT AVG(COALESCE(drift_magnitude, 0.0)) FROM drift_results",
        [],
        |row| row.get::<_, Option<f64>>(0),
    ) {
        Ok(value) => value,
        Err(err) if support::is_missing_table(&err) => None,
        Err(err) => {
            push_note(notes, format!("Failed to read drift magnitudes: {err}"));
            None
        }
    };

    match average_magnitude {
        Some(value) => (1.0 - value.clamp(0.0, 1.0)).clamp(0.0, 1.0),
        None if drift_detected == 0 => 1.0,
        None => 0.0,
    }
}

fn push_note(notes: &mut Vec<String>, note: impl Into<String>) {
    let note = note.into();
    if !notes.iter().any(|existing| existing == &note) {
        notes.push(note);
    }
}
