use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aether_config::{GraphBackend, HealthConfig, load_workspace_config};
use aether_store::{SqliteStore, Store, SurrealGraphStore, TestedByRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coupling::AnalysisError;
use crate::graph_algorithms::{
    GraphAlgorithmEdge, betweenness_centrality, connected_components, louvain_communities,
    page_rank, strongly_connected_components,
};

const HEALTH_SCHEMA_VERSION: &str = "1.0";
const DEFAULT_LIMIT: u32 = 10;
const MAX_LIMIT: u32 = 200;
const RECENCY_WINDOW_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthInclude {
    CriticalSymbols,
    Bottlenecks,
    Cycles,
    Orphans,
    RiskHotspots,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthRequest {
    #[serde(default)]
    pub include: Vec<HealthInclude>,
    #[serde(default = "default_health_limit")]
    pub limit: u32,
    #[serde(default)]
    pub min_risk: f64,
}

impl Default for HealthRequest {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            limit: default_health_limit(),
            min_risk: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthReport {
    pub schema_version: String,
    pub analysis: HealthAnalysisSummary,
    pub critical_symbols: Vec<SymbolHealthEntry>,
    pub bottlenecks: Vec<BottleneckEntry>,
    pub cycles: Vec<CycleEntry>,
    pub orphans: Vec<OrphanEntry>,
    pub risk_hotspots: Vec<RiskHotspotEntry>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthAnalysisSummary {
    pub total_symbols: u32,
    pub total_edges: u32,
    pub communities_detected: u32,
    pub cycles_detected: u32,
    pub orphaned_subgraphs: u32,
    pub analyzed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolHealthEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub pagerank: f64,
    pub betweenness: f64,
    pub dependents_count: u32,
    pub has_sir: bool,
    pub test_count: u32,
    pub drift_magnitude: f64,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BottleneckEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub betweenness: f64,
    pub pagerank: f64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSymbolRef {
    pub id: String,
    pub name: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleEntry {
    pub cycle_id: u32,
    pub symbols: Vec<HealthSymbolRef>,
    pub edge_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrphanEntry {
    pub subgraph_id: u32,
    pub symbols: Vec<HealthSymbolRef>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskHotspotEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HealthAnalyzer {
    workspace: PathBuf,
    config: HealthConfig,
    graph_backend: GraphBackend,
}

#[derive(Debug, Clone)]
struct SymbolSnapshot {
    symbol_name: String,
    file_path: String,
    last_accessed_at: Option<i64>,
    has_sir: bool,
}

#[derive(Debug, Clone)]
struct ComputedSymbol {
    symbol_id: String,
    symbol_name: String,
    file: String,
    pagerank: f64,
    betweenness: f64,
    dependents_count: u32,
    has_sir: bool,
    test_count: u32,
    drift_magnitude: f64,
    risk_score: f64,
    risk_factors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DependencyEdgeRow {
    source_id: String,
    target_id: String,
    edge_kind: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SymbolGraphRow {
    id: String,
    qualified_name: String,
    file_path: String,
}

impl HealthAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let config = load_workspace_config(&workspace)?;
        Ok(Self {
            workspace,
            config: config.health,
            graph_backend: config.storage.graph_backend,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    pub async fn analyze(&self, request: &HealthRequest) -> Result<HealthReport, AnalysisError> {
        self.analyze_internal(request, None).await
    }

    pub async fn analyze_with_graph(
        &self,
        request: &HealthRequest,
        graph: &SurrealGraphStore,
    ) -> Result<HealthReport, AnalysisError> {
        self.analyze_internal(request, Some(graph)).await
    }

    async fn analyze_internal(
        &self,
        request: &HealthRequest,
        graph_override: Option<&SurrealGraphStore>,
    ) -> Result<HealthReport, AnalysisError> {
        let analyzed_at = now_millis();
        let mut notes = Vec::new();
        let include = effective_include_set(request.include.as_slice());
        let limit = effective_limit(request.limit);
        let min_risk = request.min_risk.clamp(0.0, 1.0);

        if !self.config.enabled {
            notes.push(
                "Health analysis disabled by configuration ([health].enabled = false)".to_owned(),
            );
            return Ok(empty_report(analyzed_at, notes));
        }

        if graph_override.is_none() && self.graph_backend != GraphBackend::Surreal {
            notes.push(format!(
                "Health analysis currently requires Surreal graph backend; configured backend is {}",
                self.graph_backend.as_str()
            ));
            return Ok(empty_report(analyzed_at, notes));
        }

        let store = SqliteStore::open(&self.workspace)?;
        let opened_graph = if graph_override.is_some() {
            None
        } else {
            match SurrealGraphStore::open_readonly(&self.workspace).await {
                Ok(graph) => Some(graph),
                Err(err) => {
                    notes.push(format!(
                        "Surreal graph store unavailable; returning empty health report: {err}"
                    ));
                    return Ok(empty_report(analyzed_at, notes));
                }
            }
        };
        let graph = if let Some(existing) = graph_override {
            existing
        } else if let Some(opened) = opened_graph.as_ref() {
            opened
        } else {
            notes.push("Surreal graph store unavailable; returning empty health report".to_owned());
            return Ok(empty_report(analyzed_at, notes));
        };

        let dependency_edges = match query_dependency_edges(graph).await {
            Ok(edges) => edges,
            Err(err) => {
                notes.push(format!(
                    "Dependency edge query failed; returning empty health report: {err}"
                ));
                return Ok(empty_report(analyzed_at, notes));
            }
        };

        let symbol_rows = match query_symbol_rows(graph).await {
            Ok(rows) => rows,
            Err(err) => {
                notes.push(format!(
                    "Symbol query failed; returning empty health report: {err}"
                ));
                return Ok(empty_report(analyzed_at, notes));
            }
        };

        let tested_by_rows = match query_tested_by_rows(graph).await {
            Ok(rows) => rows,
            Err(err) => {
                notes.push(format!(
                    "tested_by query unavailable ({err}); falling back to test_intents"
                ));
                Vec::new()
            }
        };

        if dependency_edges.is_empty() {
            notes.push("No dependency edges found in SurrealDB".to_owned());
        }

        let algo_edges = dependency_edges.clone();
        let (pagerank_scores, betweenness_scores, scc, cc, communities) =
            tokio::task::spawn_blocking(move || {
                (
                    page_rank(&algo_edges, 0.85, 25),
                    betweenness_centrality(&algo_edges)
                        .into_iter()
                        .collect::<HashMap<_, _>>(),
                    strongly_connected_components(&algo_edges),
                    connected_components(&algo_edges),
                    louvain_communities(&algo_edges),
                )
            })
            .await
            .map_err(|err| AnalysisError::Message(format!("spawn_blocking failed: {err}")))?;

        let mut symbol_ids = BTreeSet::<String>::new();
        for symbol in &symbol_rows {
            symbol_ids.insert(symbol.id.clone());
        }
        for edge in &dependency_edges {
            symbol_ids.insert(edge.source_id.clone());
            symbol_ids.insert(edge.target_id.clone());
        }

        let symbol_ids_vec = symbol_ids.into_iter().collect::<Vec<_>>();
        let sqlite_rows = store.get_symbol_search_results_batch(symbol_ids_vec.as_slice())?;
        let drift_by_symbol = latest_semantic_drift_by_symbol(&store)?;

        let mut symbol_graph_by_id = HashMap::<String, SymbolGraphRow>::new();
        for row in symbol_rows {
            symbol_graph_by_id.insert(row.id.clone(), row);
        }

        let mut symbol_snapshot_by_id = HashMap::<String, SymbolSnapshot>::new();
        for symbol_id in &symbol_ids_vec {
            let sqlite = sqlite_rows.get(symbol_id);
            let graph_row = symbol_graph_by_id.get(symbol_id);
            let fallback = store.get_symbol_record(symbol_id)?;

            let symbol_name = sqlite
                .map(|row| row.qualified_name.clone())
                .or_else(|| graph_row.map(|row| row.qualified_name.clone()))
                .or_else(|| fallback.as_ref().map(|row| row.qualified_name.clone()))
                .unwrap_or_else(|| symbol_id.clone());
            let file_path = sqlite
                .map(|row| row.file_path.clone())
                .or_else(|| graph_row.map(|row| row.file_path.clone()))
                .or_else(|| fallback.as_ref().map(|row| row.file_path.clone()))
                .unwrap_or_default();
            let last_accessed_at = sqlite.and_then(|row| row.last_accessed_at);
            let has_sir = has_sir(&store, symbol_id)?;

            symbol_snapshot_by_id.insert(
                symbol_id.clone(),
                SymbolSnapshot {
                    symbol_name,
                    file_path,
                    last_accessed_at,
                    has_sir,
                },
            );
        }

        let dependents_by_symbol = dependency_edges.iter().fold(
            HashMap::<String, HashSet<String>>::new(),
            |mut acc, edge| {
                acc.entry(edge.target_id.clone())
                    .or_default()
                    .insert(edge.source_id.clone());
                acc
            },
        );

        let mut symbols_by_file = HashMap::<String, Vec<String>>::new();
        for (symbol_id, snapshot) in &symbol_snapshot_by_id {
            if snapshot.file_path.is_empty() {
                continue;
            }
            symbols_by_file
                .entry(snapshot.file_path.clone())
                .or_default()
                .push(symbol_id.clone());
        }

        let mut test_count_by_symbol = HashMap::<String, u32>::new();
        let mut use_test_intents_fallback = tested_by_rows.is_empty();

        if !use_test_intents_fallback {
            for row in tested_by_rows {
                let Some(symbols) = symbols_by_file.get(row.target_file.as_str()) else {
                    continue;
                };
                for symbol_id in symbols {
                    *test_count_by_symbol.entry(symbol_id.clone()).or_insert(0) += 1;
                }
            }

            if test_count_by_symbol.is_empty()
                || test_count_by_symbol.values().all(|count| *count == 0)
            {
                use_test_intents_fallback = true;
                notes.push(
                    "tested_by produced no symbol coverage; using test_intents fallback".to_owned(),
                );
            }
        }

        if use_test_intents_fallback {
            if !notes
                .iter()
                .any(|note| note.contains("test_intents fallback"))
            {
                notes.push("Using test_intents fallback for test coverage".to_owned());
            }
            test_count_by_symbol.clear();
            for symbol_id in &symbol_ids_vec {
                let count = store.list_test_intents_for_symbol(symbol_id)?.len() as u32;
                test_count_by_symbol.insert(symbol_id.clone(), count);
            }
        }

        let max_pagerank = pagerank_scores.values().copied().fold(0.0f64, f64::max);

        let mut computed_symbols = Vec::<ComputedSymbol>::new();
        for symbol_id in &symbol_ids_vec {
            let snapshot =
                symbol_snapshot_by_id
                    .get(symbol_id)
                    .cloned()
                    .unwrap_or(SymbolSnapshot {
                        symbol_name: symbol_id.clone(),
                        file_path: String::new(),
                        last_accessed_at: None,
                        has_sir: false,
                    });

            let pagerank = pagerank_scores.get(symbol_id).copied().unwrap_or(0.0);
            let betweenness = betweenness_scores.get(symbol_id).copied().unwrap_or(0.0);
            let dependents_count = dependents_by_symbol
                .get(symbol_id)
                .map(|entries| entries.len() as u32)
                .unwrap_or(0);
            let test_count = test_count_by_symbol.get(symbol_id).copied().unwrap_or(0);
            let test_coverage_ratio = ((test_count as f64) / 3.0).min(1.0);
            let drift_magnitude = drift_by_symbol.get(symbol_id).copied().unwrap_or(0.0);
            let access_recency_factor = recency_factor(snapshot.last_accessed_at, analyzed_at);
            let pagerank_normalized = if max_pagerank > f64::EPSILON {
                pagerank / max_pagerank
            } else {
                0.0
            };

            let no_sir_factor = if snapshot.has_sir { 0.0 } else { 1.0 };
            let contribution_pagerank = self.config.risk_weights.pagerank * pagerank_normalized;
            let contribution_test_gap =
                self.config.risk_weights.test_gap * (1.0 - test_coverage_ratio);
            let contribution_drift = self.config.risk_weights.drift * drift_magnitude;
            let contribution_no_sir = self.config.risk_weights.no_sir * no_sir_factor;
            let contribution_recency = self.config.risk_weights.recency * access_recency_factor;

            let risk_score = (contribution_pagerank
                + contribution_test_gap
                + contribution_drift
                + contribution_no_sir
                + contribution_recency)
                .clamp(0.0, 1.0);

            let mut risk_factors = Vec::<String>::new();
            if contribution_pagerank > 0.1 {
                risk_factors.push(format!("high pagerank {:.2}", pagerank));
            }
            if contribution_test_gap > 0.1 {
                risk_factors.push(format!("low test coverage ({test_count} test guards)"));
            }
            if contribution_drift > 0.1 {
                risk_factors.push(format!("semantic drift {:.2}", drift_magnitude));
            }
            if contribution_no_sir > 0.1 {
                risk_factors.push("no SIR metadata".to_owned());
            }
            if contribution_recency > 0.1 {
                risk_factors.push("not accessed recently".to_owned());
            }

            computed_symbols.push(ComputedSymbol {
                symbol_id: symbol_id.clone(),
                symbol_name: snapshot.symbol_name,
                file: snapshot.file_path,
                pagerank,
                betweenness,
                dependents_count,
                has_sir: snapshot.has_sir,
                test_count,
                drift_magnitude,
                risk_score,
                risk_factors,
            });
        }

        computed_symbols.sort_by(|left, right| {
            right
                .risk_score
                .partial_cmp(&left.risk_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let mut cycles_all = scc
            .into_iter()
            .filter(|component| component.len() > 1)
            .enumerate()
            .map(|(idx, component)| {
                let ids = component.into_iter().collect::<HashSet<_>>();
                let mut symbols = ids
                    .iter()
                    .map(|id| to_symbol_ref(id, &symbol_snapshot_by_id))
                    .collect::<Vec<_>>();
                symbols.sort_by(|left, right| left.id.cmp(&right.id));

                let edge_count = dependency_edges
                    .iter()
                    .filter(|edge| {
                        ids.contains(edge.source_id.as_str())
                            && ids.contains(edge.target_id.as_str())
                    })
                    .count() as u32;

                CycleEntry {
                    cycle_id: (idx + 1) as u32,
                    symbols,
                    edge_count,
                    note: "Circular dependency cluster detected".to_owned(),
                }
            })
            .collect::<Vec<_>>();

        cycles_all.sort_by(|left, right| {
            right
                .symbols
                .len()
                .cmp(&left.symbols.len())
                .then_with(|| left.cycle_id.cmp(&right.cycle_id))
        });

        let mut orphan_components = cc;
        if !orphan_components.is_empty() {
            orphan_components.remove(0);
        }
        let mut orphans_all = orphan_components
            .into_iter()
            .filter(|component| !component.is_empty())
            .enumerate()
            .map(|(idx, component)| {
                let mut symbols = component
                    .iter()
                    .map(|id| to_symbol_ref(id, &symbol_snapshot_by_id))
                    .collect::<Vec<_>>();
                symbols.sort_by(|left, right| left.id.cmp(&right.id));

                OrphanEntry {
                    subgraph_id: (idx + 1) as u32,
                    symbols,
                    note: "No dependency edges to main component; orphaned code candidate"
                        .to_owned(),
                }
            })
            .collect::<Vec<_>>();

        orphans_all.sort_by(|left, right| {
            right
                .symbols
                .len()
                .cmp(&left.symbols.len())
                .then_with(|| left.subgraph_id.cmp(&right.subgraph_id))
        });

        let mut bottlenecks_all = computed_symbols
            .iter()
            .filter(|row| row.betweenness > 0.0)
            .map(|row| BottleneckEntry {
                symbol_id: row.symbol_id.clone(),
                symbol_name: row.symbol_name.clone(),
                file: row.file.clone(),
                betweenness: row.betweenness,
                pagerank: row.pagerank,
                note: format!(
                    "{:.0}% of shortest dependency paths pass through this symbol",
                    row.betweenness * 100.0
                ),
            })
            .collect::<Vec<_>>();

        bottlenecks_all.sort_by(|left, right| {
            right
                .betweenness
                .partial_cmp(&left.betweenness)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    right
                        .pagerank
                        .partial_cmp(&left.pagerank)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let critical_symbols_all = computed_symbols
            .iter()
            .filter(|row| row.risk_score >= min_risk)
            .map(|row| SymbolHealthEntry {
                symbol_id: row.symbol_id.clone(),
                symbol_name: row.symbol_name.clone(),
                file: row.file.clone(),
                pagerank: row.pagerank,
                betweenness: row.betweenness,
                dependents_count: row.dependents_count,
                has_sir: row.has_sir,
                test_count: row.test_count,
                drift_magnitude: row.drift_magnitude,
                risk_score: row.risk_score,
                risk_factors: row.risk_factors.clone(),
            })
            .collect::<Vec<_>>();

        let risk_hotspots_all = computed_symbols
            .iter()
            .filter(|row| row.risk_score >= min_risk)
            .map(|row| RiskHotspotEntry {
                symbol_id: row.symbol_id.clone(),
                symbol_name: row.symbol_name.clone(),
                file: row.file.clone(),
                risk_score: row.risk_score,
                risk_factors: row.risk_factors.clone(),
            })
            .collect::<Vec<_>>();

        let communities_detected =
            communities.values().copied().collect::<HashSet<_>>().len() as u32;

        let analysis = HealthAnalysisSummary {
            total_symbols: symbol_ids_vec.len() as u32,
            total_edges: dependency_edges.len() as u32,
            communities_detected,
            cycles_detected: cycles_all.len() as u32,
            orphaned_subgraphs: orphans_all.len() as u32,
            analyzed_at,
        };

        let critical_symbols = if include.contains(&HealthInclude::CriticalSymbols) {
            critical_symbols_all
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let bottlenecks = if include.contains(&HealthInclude::Bottlenecks) {
            bottlenecks_all
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let cycles = if include.contains(&HealthInclude::Cycles) {
            cycles_all
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let orphans = if include.contains(&HealthInclude::Orphans) {
            orphans_all
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let risk_hotspots = if include.contains(&HealthInclude::RiskHotspots) {
            risk_hotspots_all
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        Ok(HealthReport {
            schema_version: HEALTH_SCHEMA_VERSION.to_owned(),
            analysis,
            critical_symbols,
            bottlenecks,
            cycles,
            orphans,
            risk_hotspots,
            notes,
        })
    }
}

fn default_health_limit() -> u32 {
    DEFAULT_LIMIT
}

fn effective_limit(raw: u32) -> u32 {
    if raw == 0 {
        DEFAULT_LIMIT
    } else {
        raw.clamp(1, MAX_LIMIT)
    }
}

fn effective_include_set(raw: &[HealthInclude]) -> HashSet<HealthInclude> {
    if raw.is_empty() {
        return [
            HealthInclude::CriticalSymbols,
            HealthInclude::Bottlenecks,
            HealthInclude::Cycles,
            HealthInclude::Orphans,
            HealthInclude::RiskHotspots,
        ]
        .into_iter()
        .collect();
    }

    raw.iter().copied().collect()
}

fn empty_report(analyzed_at: i64, notes: Vec<String>) -> HealthReport {
    HealthReport {
        schema_version: HEALTH_SCHEMA_VERSION.to_owned(),
        analysis: HealthAnalysisSummary {
            total_symbols: 0,
            total_edges: 0,
            communities_detected: 0,
            cycles_detected: 0,
            orphaned_subgraphs: 0,
            analyzed_at,
        },
        critical_symbols: Vec::new(),
        bottlenecks: Vec::new(),
        cycles: Vec::new(),
        orphans: Vec::new(),
        risk_hotspots: Vec::new(),
        notes,
    }
}

fn to_symbol_ref(symbol_id: &str, snapshots: &HashMap<String, SymbolSnapshot>) -> HealthSymbolRef {
    if let Some(snapshot) = snapshots.get(symbol_id) {
        HealthSymbolRef {
            id: symbol_id.to_owned(),
            name: snapshot.symbol_name.clone(),
            file: snapshot.file_path.clone(),
        }
    } else {
        HealthSymbolRef {
            id: symbol_id.to_owned(),
            name: symbol_id.to_owned(),
            file: String::new(),
        }
    }
}

fn has_sir(store: &SqliteStore, symbol_id: &str) -> Result<bool, AnalysisError> {
    if let Some(meta) = store.get_sir_meta(symbol_id)?
        && meta.sir_status.trim().eq_ignore_ascii_case("ready")
    {
        return Ok(true);
    }

    Ok(store
        .read_sir_blob(symbol_id)?
        .is_some_and(|value| !value.trim().is_empty()))
}

fn latest_semantic_drift_by_symbol(
    store: &SqliteStore,
) -> Result<HashMap<String, f64>, AnalysisError> {
    let records = store.list_drift_results(true)?;
    let mut by_symbol = HashMap::new();
    for record in records {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };
        by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(by_symbol)
}

fn recency_factor(last_accessed_at: Option<i64>, now_ms: i64) -> f64 {
    let Some(raw) = last_accessed_at else {
        return 1.0;
    };

    let timestamp_ms = if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw
    };

    let age_ms = now_ms.saturating_sub(timestamp_ms).max(0);
    let max_window = RECENCY_WINDOW_DAYS.saturating_mul(DAY_MS) as f64;
    if max_window <= f64::EPSILON {
        return 0.0;
    }

    (age_ms as f64 / max_window).clamp(0.0, 1.0)
}

async fn query_dependency_edges(
    graph: &SurrealGraphStore,
) -> Result<Vec<GraphAlgorithmEdge>, AnalysisError> {
    let mut response = graph
        .db()
        .query(
            r#"
            SELECT VALUE {
                source_id: source_symbol_id,
                target_id: target_symbol_id,
                edge_kind: edge_kind
            }
            FROM depends_on
            WHERE edge_kind INSIDE ["calls", "depends_on"];
            "#,
        )
        .await
        .map_err(|err| AnalysisError::Message(format!("surreal dependency query failed: {err}")))?;

    let rows: Vec<Value> = response.take(0).map_err(|err| {
        AnalysisError::Message(format!("surreal dependency decode failed: {err}"))
    })?;
    let mut decoded = decode_rows::<DependencyEdgeRow>(rows)?;
    decoded.retain(|row| !row.source_id.is_empty() && !row.target_id.is_empty());

    Ok(decoded
        .into_iter()
        .map(|row| GraphAlgorithmEdge {
            source_id: row.source_id,
            target_id: row.target_id,
            edge_kind: row.edge_kind,
        })
        .collect())
}

async fn query_symbol_rows(
    graph: &SurrealGraphStore,
) -> Result<Vec<SymbolGraphRow>, AnalysisError> {
    let mut response = graph
        .db()
        .query(
            r#"
            SELECT VALUE {
                id: symbol_id,
                qualified_name: qualified_name,
                file_path: file_path
            }
            FROM symbol;
            "#,
        )
        .await
        .map_err(|err| AnalysisError::Message(format!("surreal symbol query failed: {err}")))?;

    let rows: Vec<Value> = response
        .take(0)
        .map_err(|err| AnalysisError::Message(format!("surreal symbol decode failed: {err}")))?;
    let mut decoded = decode_rows::<SymbolGraphRow>(rows)?;
    decoded.retain(|row| !row.id.is_empty());
    Ok(decoded)
}

async fn query_tested_by_rows(
    graph: &SurrealGraphStore,
) -> Result<Vec<TestedByRecord>, AnalysisError> {
    let mut response = graph
        .db()
        .query(
            r#"
            SELECT VALUE {
                target_file: target_file,
                test_file: test_file,
                intent_count: intent_count,
                confidence: confidence,
                inference_method: inference_method
            }
            FROM tested_by;
            "#,
        )
        .await
        .map_err(|err| AnalysisError::Message(format!("surreal tested_by query failed: {err}")))?;

    let rows: Vec<Value> = response
        .take(0)
        .map_err(|err| AnalysisError::Message(format!("surreal tested_by decode failed: {err}")))?;
    decode_rows(rows)
}

fn decode_rows<T: for<'de> Deserialize<'de>>(rows: Vec<Value>) -> Result<Vec<T>, AnalysisError> {
    rows.into_iter()
        .map(|row| serde_json::from_value(row).map_err(AnalysisError::from))
        .collect::<Result<Vec<_>, _>>()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::EdgeKind;
    use aether_store::{
        DriftResultRecord, GraphStore, ResolvedEdge, SirMetaRecord, SqliteStore, Store,
        SurrealGraphStore, SymbolRecord, TestIntentRecord,
    };
    use tempfile::tempdir;

    use super::{HealthAnalyzer, HealthInclude, HealthRequest, now_millis};

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "surreal"

[embeddings]
enabled = false
provider = "mock"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn symbol(id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: now_millis(),
        }
    }

    fn insert_test_intents(
        store: &aether_store::SqliteStore,
        symbol_id: &str,
        file: &str,
        count: u32,
    ) {
        let intents = (0..count)
            .map(|idx| TestIntentRecord {
                intent_id: format!("intent-{symbol_id}-{idx}"),
                file_path: file.to_owned(),
                test_name: format!("test_{symbol_id}_{idx}"),
                intent_text: format!("covers {symbol_id} #{idx}"),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: Some(symbol_id.to_owned()),
                created_at: now_millis(),
                updated_at: now_millis(),
            })
            .collect::<Vec<_>>();
        store
            .replace_test_intents_for_file(file, intents.as_slice())
            .expect("replace test intents");
    }

    async fn seed_surreal_graph(
        workspace: &Path,
        symbols: &[SymbolRecord],
        edges: &[(&str, &str)],
    ) {
        let graph = SurrealGraphStore::open(workspace)
            .await
            .expect("open surreal");

        for symbol in symbols {
            graph
                .upsert_symbol_node(symbol)
                .await
                .expect("upsert symbol node");
        }

        for (source, target) in edges {
            graph
                .upsert_edge(&ResolvedEdge {
                    source_id: (*source).to_owned(),
                    target_id: (*target).to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/graph.rs".to_owned(),
                })
                .await
                .expect("upsert edge");
        }
    }

    #[test]
    fn health_analyzer_computes_risk_and_fallback_test_coverage() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let store = SqliteStore::open(workspace).expect("open sqlite");
        let symbols = vec![
            symbol("sym-a", "crate::alpha", "src/a.rs"),
            symbol("sym-b", "crate::beta", "src/b.rs"),
            symbol("sym-c", "crate::gamma", "src/c.rs"),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }

        insert_test_intents(&store, "sym-a", "tests/a_test.rs", 3);
        insert_test_intents(&store, "sym-c", "tests/c_test.rs", 1);

        store
            .upsert_sir_meta(SirMetaRecord {
                id: "sym-a".to_owned(),
                sir_hash: "hash-a".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: now_millis(),
                sir_status: "ready".to_owned(),
                last_error: None,
                last_attempt_at: now_millis(),
            })
            .expect("sir meta a");

        store
            .upsert_sir_meta(SirMetaRecord {
                id: "sym-c".to_owned(),
                sir_hash: "hash-c".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: now_millis(),
                sir_status: "ready".to_owned(),
                last_error: None,
                last_attempt_at: now_millis(),
            })
            .expect("sir meta c");

        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-sym-b".to_owned(),
                symbol_id: "sym-b".to_owned(),
                file_path: "src/b.rs".to_owned(),
                symbol_name: "crate::beta".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.8),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some("a1".to_owned()),
                commit_range_end: Some("b2".to_owned()),
                drift_summary: Some("beta changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: now_millis(),
                is_acknowledged: false,
            }])
            .expect("seed drift");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(seed_surreal_graph(
            workspace,
            &symbols,
            &[("sym-a", "sym-b"), ("sym-b", "sym-c")],
        ));

        let analyzer = HealthAnalyzer::new(workspace).expect("analyzer");

        let report = runtime
            .block_on(analyzer.analyze(&HealthRequest::default()))
            .expect("health report");

        assert_eq!(report.analysis.total_symbols, 3);
        assert_eq!(report.analysis.total_edges, 2);
        assert!(!report.critical_symbols.is_empty());

        let alpha = report
            .critical_symbols
            .iter()
            .find(|entry| entry.symbol_id == "sym-a")
            .expect("alpha entry");
        assert_eq!(alpha.test_count, 3);

        let beta = report
            .critical_symbols
            .iter()
            .find(|entry| entry.symbol_id == "sym-b")
            .expect("beta entry");
        assert!(!beta.has_sir);
        assert!(
            beta.risk_factors
                .iter()
                .any(|factor| factor.contains("no SIR metadata"))
        );

        let filtered = runtime
            .block_on(analyzer.analyze(&HealthRequest {
                include: Vec::new(),
                limit: 10,
                min_risk: (beta.risk_score + 0.01).clamp(0.0, 1.0),
            }))
            .expect("filtered health report");
        assert!(
            filtered
                .critical_symbols
                .iter()
                .all(|entry| entry.risk_score >= (beta.risk_score + 0.01).clamp(0.0, 1.0))
        );
    }

    #[test]
    fn health_analyzer_include_cycles_only() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let store = SqliteStore::open(workspace).expect("open sqlite");
        let symbols = vec![
            symbol("sym-a", "crate::alpha", "src/a.rs"),
            symbol("sym-b", "crate::beta", "src/b.rs"),
            symbol("sym-c", "crate::gamma", "src/c.rs"),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(seed_surreal_graph(
            workspace,
            &symbols,
            &[("sym-a", "sym-b"), ("sym-b", "sym-c"), ("sym-c", "sym-a")],
        ));
        let analyzer = HealthAnalyzer::new(workspace).expect("analyzer");

        let report = runtime
            .block_on(analyzer.analyze(&HealthRequest {
                include: vec![HealthInclude::Cycles],
                limit: 10,
                min_risk: 0.0,
            }))
            .expect("health report");

        assert!(!report.cycles.is_empty());
        assert!(report.critical_symbols.is_empty());
        assert!(report.bottlenecks.is_empty());
        assert!(report.orphans.is_empty());
        assert!(report.risk_hotspots.is_empty());
    }

    #[test]
    fn health_analyzer_empty_graph_returns_empty_sections() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let analyzer = HealthAnalyzer::new(workspace).expect("analyzer");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let report = runtime
            .block_on(analyzer.analyze(&HealthRequest::default()))
            .expect("health report");

        assert_eq!(report.analysis.total_edges, 0);
        assert!(report.critical_symbols.is_empty());
        assert!(report.bottlenecks.is_empty());
        assert!(report.cycles.is_empty());
        assert!(report.orphans.is_empty());
        assert!(report.risk_hotspots.is_empty());
    }
}
