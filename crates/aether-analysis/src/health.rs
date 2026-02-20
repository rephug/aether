use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{HealthConfig, HealthRiskWeights, load_workspace_config};
use aether_store::{CozoGraphStore, DriftResultRecord, SqliteStore, Store, SymbolRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coupling::AnalysisError;

const HEALTH_SCHEMA_VERSION: &str = "1.0";
const RISK_FACTOR_MIN_CONTRIBUTION: f32 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthInclude {
    CriticalSymbols,
    Bottlenecks,
    Cycles,
    Orphans,
    RiskHotspots,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HealthReportRequest {
    pub include: Option<Vec<HealthInclude>>,
    pub limit: Option<u32>,
    pub min_risk: Option<f32>,
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
pub struct HealthSymbolEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub pagerank: f32,
    pub betweenness: f32,
    pub dependents_count: u32,
    pub has_sir: bool,
    pub test_count: u32,
    pub drift_magnitude: f32,
    pub risk_score: f32,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthBottleneckEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub betweenness: f32,
    pub pagerank: f32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCycleSymbol {
    pub id: String,
    pub name: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCycleEntry {
    pub cycle_id: u32,
    pub symbols: Vec<HealthCycleSymbol>,
    pub edge_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthOrphanEntry {
    pub subgraph_id: u32,
    pub symbols: Vec<HealthCycleSymbol>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthRiskHotspotEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub risk_score: f32,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthReportResult {
    pub schema_version: String,
    pub analysis: HealthAnalysisSummary,
    pub critical_symbols: Vec<HealthSymbolEntry>,
    pub bottlenecks: Vec<HealthBottleneckEntry>,
    pub cycles: Vec<HealthCycleEntry>,
    pub orphans: Vec<HealthOrphanEntry>,
    pub risk_hotspots: Vec<HealthRiskHotspotEntry>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HealthAnalyzer {
    workspace: PathBuf,
    config: HealthConfig,
}

#[derive(Debug, Clone)]
struct SymbolHealthContext {
    symbol: SymbolRecord,
    pagerank: f32,
    pagerank_normalized: f32,
    pagerank_percentile: u32,
    betweenness: f32,
    dependents_count: u32,
    test_count: u32,
    test_coverage_ratio: f32,
    drift_magnitude: f32,
    has_sir: bool,
    edge_case_count: u32,
    access_recency_factor: f32,
    boundary_violations: u32,
}

#[derive(Debug, Clone)]
struct RiskContribution {
    label: &'static str,
    contribution: f32,
    message: String,
}

impl HealthAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let config = load_workspace_config(&workspace)?;
        Ok(Self {
            workspace,
            config: config.health,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    pub fn report(
        &self,
        request: HealthReportRequest,
    ) -> Result<HealthReportResult, AnalysisError> {
        let analyzed_at = now_millis();
        let limit = request.limit.unwrap_or(10).clamp(1, 200);
        let min_risk = request.min_risk.unwrap_or(0.5).clamp(0.0, 1.0);
        let includes = effective_includes(request.include.as_deref());

        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;

        let mut notes = Vec::new();
        let symbols = store.list_symbols()?;
        let by_symbol = symbols
            .iter()
            .cloned()
            .map(|symbol| (symbol.id.clone(), symbol))
            .collect::<HashMap<_, _>>();
        let edges = cozo.list_dependency_edges()?;
        let total_edges = edges.len() as u32;

        if !self.config.enabled {
            notes.push("health analysis disabled by config [health].enabled=false".to_owned());
            return Ok(HealthReportResult {
                schema_version: HEALTH_SCHEMA_VERSION.to_owned(),
                analysis: HealthAnalysisSummary {
                    total_symbols: symbols.len() as u32,
                    total_edges,
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
            });
        }

        if edges.is_empty() {
            notes.push("no dependency edges found; graph sections are empty".to_owned());
        }

        let pagerank = cozo.list_pagerank()?;
        let mut pagerank_by_symbol = pagerank.into_iter().collect::<HashMap<_, _>>();
        let max_pagerank = pagerank_by_symbol
            .values()
            .copied()
            .fold(0.0f32, f32::max)
            .max(1e-6);

        let mut pagerank_ranking = pagerank_by_symbol
            .iter()
            .map(|(symbol_id, score)| (symbol_id.clone(), *score))
            .collect::<Vec<_>>();
        pagerank_ranking.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        let pagerank_rank_index = pagerank_ranking
            .iter()
            .enumerate()
            .map(|(index, (symbol_id, _))| (symbol_id.clone(), index as u32))
            .collect::<HashMap<_, _>>();

        let betweenness = cozo.list_betweenness_centrality()?;
        let betweenness_by_symbol = betweenness.into_iter().collect::<HashMap<_, _>>();

        let louvain = cozo.list_louvain_communities()?;
        let community_by_symbol = louvain.into_iter().collect::<HashMap<_, _>>();
        let communities_detected = community_by_symbol
            .values()
            .copied()
            .collect::<HashSet<_>>()
            .len() as u32;

        let scc = cozo.list_strongly_connected_components()?;
        let cycles = scc
            .iter()
            .filter(|component| component.len() > 1)
            .cloned()
            .collect::<Vec<_>>();

        let connected_components = cozo.list_connected_components()?;
        let largest_component = connected_components
            .iter()
            .max_by_key(|component| component.len())
            .cloned()
            .unwrap_or_default();
        let largest_component_set = largest_component.into_iter().collect::<HashSet<_>>();
        let component_symbols = connected_components
            .iter()
            .flatten()
            .cloned()
            .collect::<HashSet<_>>();
        let mut orphan_components = connected_components
            .into_iter()
            .filter(|component| {
                if component.is_empty() {
                    return false;
                }
                component
                    .iter()
                    .any(|symbol_id| !largest_component_set.contains(symbol_id))
                    && !component
                        .iter()
                        .any(|symbol_id| largest_component_set.contains(symbol_id))
            })
            .collect::<Vec<_>>();
        if !edges.is_empty() {
            for symbol_id in by_symbol.keys() {
                if component_symbols.contains(symbol_id) {
                    continue;
                }
                orphan_components.push(vec![symbol_id.clone()]);
            }
        }
        for component in &mut orphan_components {
            component.sort();
        }
        orphan_components.sort_by(|left, right| {
            left.first()
                .cmp(&right.first())
                .then_with(|| left.len().cmp(&right.len()))
        });

        let cross_community_edges = cozo.list_cross_community_edges(&community_by_symbol)?;
        let mut boundary_violations_by_symbol = HashMap::<String, BTreeSet<i64>>::new();
        for (source_id, _target_id, _edge_kind, _source_community, target_community) in
            cross_community_edges
        {
            boundary_violations_by_symbol
                .entry(source_id)
                .or_default()
                .insert(target_community);
        }
        let boundary_violations_by_symbol = boundary_violations_by_symbol
            .into_iter()
            .map(|(symbol_id, communities)| (symbol_id, communities.len() as u32))
            .collect::<HashMap<_, _>>();

        let mut dependents_count_by_symbol = HashMap::<String, u32>::new();
        for (_source, target, _kind) in &edges {
            *dependents_count_by_symbol
                .entry(target.clone())
                .or_insert(0) += 1;
        }

        let semantic_drift = store
            .list_drift_results(true)?
            .into_iter()
            .filter(|record| record.drift_type == "semantic")
            .collect::<Vec<_>>();
        let drift_by_symbol = latest_drift_by_symbol(semantic_drift.as_slice());

        let unique_files = symbols
            .iter()
            .map(|symbol| symbol.file_path.clone())
            .collect::<BTreeSet<_>>();
        let mut tests_by_file = HashMap::<String, u32>::new();
        for file in unique_files {
            let guards = cozo.list_tested_by_for_target_file(file.as_str())?;
            let unique_guards = guards
                .iter()
                .map(|guard| guard.test_file.clone())
                .collect::<HashSet<_>>();
            tests_by_file.insert(file, unique_guards.len() as u32);
        }

        let mut symbol_contexts = Vec::new();
        for symbol in symbols {
            let pagerank_value = pagerank_by_symbol.remove(symbol.id.as_str()).unwrap_or(0.0);
            let rank_index = pagerank_rank_index
                .get(symbol.id.as_str())
                .copied()
                .unwrap_or((by_symbol.len() as u32).saturating_sub(1));
            let percentile = if by_symbol.is_empty() {
                100
            } else {
                ((rank_index + 1) * 100).saturating_div(by_symbol.len() as u32)
            };
            let test_count = tests_by_file
                .get(symbol.file_path.as_str())
                .copied()
                .unwrap_or(0);
            let sir = store.read_sir_blob(symbol.id.as_str())?;
            let has_sir = sir.is_some();
            let edge_case_count = sir.as_deref().map(parse_edge_case_count).unwrap_or(0);
            let test_coverage_ratio = if edge_case_count > 0 {
                (test_count as f32 / edge_case_count as f32).clamp(0.0, 1.0)
            } else if test_count > 0 {
                1.0
            } else {
                0.0
            };

            let access_recency_factor = store
                .get_symbol_search_result(symbol.id.as_str())?
                .and_then(|result| result.last_accessed_at)
                .map(recency_factor)
                .unwrap_or(0.0);

            symbol_contexts.push(SymbolHealthContext {
                symbol: symbol.clone(),
                pagerank: pagerank_value,
                pagerank_normalized: (pagerank_value / max_pagerank).clamp(0.0, 1.0),
                pagerank_percentile: percentile.max(1),
                betweenness: betweenness_by_symbol
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0),
                dependents_count: dependents_count_by_symbol
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0),
                test_count,
                test_coverage_ratio,
                drift_magnitude: drift_by_symbol
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0),
                has_sir,
                edge_case_count,
                access_recency_factor,
                boundary_violations: boundary_violations_by_symbol
                    .get(symbol.id.as_str())
                    .copied()
                    .unwrap_or(0),
            });
        }

        let mut scored_symbols = symbol_contexts
            .iter()
            .map(|context| {
                let (score, factors) = compute_risk_score(context, &self.config.risk_weights);
                (context.clone(), score, factors)
            })
            .collect::<Vec<_>>();

        scored_symbols.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.symbol.id.cmp(&right.0.symbol.id))
        });

        let critical_symbols = if includes.contains(&HealthInclude::CriticalSymbols) {
            scored_symbols
                .iter()
                .filter(|(_, score, _)| *score >= min_risk)
                .take(limit as usize)
                .map(|(context, score, factors)| HealthSymbolEntry {
                    symbol_id: context.symbol.id.clone(),
                    symbol_name: symbol_leaf_name(context.symbol.qualified_name.as_str()),
                    file: context.symbol.file_path.clone(),
                    pagerank: context.pagerank,
                    betweenness: context.betweenness,
                    dependents_count: context.dependents_count,
                    has_sir: context.has_sir,
                    test_count: context.test_count,
                    drift_magnitude: context.drift_magnitude,
                    risk_score: *score,
                    risk_factors: factors.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        let bottlenecks = if includes.contains(&HealthInclude::Bottlenecks) {
            let mut top = symbol_contexts
                .iter()
                .filter(|context| context.betweenness > 0.0)
                .collect::<Vec<_>>();
            top.sort_by(|left, right| {
                right
                    .betweenness
                    .partial_cmp(&left.betweenness)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.symbol.id.cmp(&right.symbol.id))
            });
            top.into_iter()
                .take(limit as usize)
                .map(|context| HealthBottleneckEntry {
                    symbol_id: context.symbol.id.clone(),
                    symbol_name: symbol_leaf_name(context.symbol.qualified_name.as_str()),
                    file: context.symbol.file_path.clone(),
                    betweenness: context.betweenness,
                    pagerank: context.pagerank,
                    note: format!(
                        "{:.0}% of dependency paths pass through this symbol",
                        (context.betweenness * 100.0).clamp(0.0, 100.0)
                    ),
                })
                .collect()
        } else {
            Vec::new()
        };

        let cycles_output = if includes.contains(&HealthInclude::Cycles) {
            cycles
                .iter()
                .enumerate()
                .map(|(index, component)| {
                    let symbols = component
                        .iter()
                        .filter_map(|symbol_id| by_symbol.get(symbol_id.as_str()))
                        .map(|symbol| HealthCycleSymbol {
                            id: symbol.id.clone(),
                            name: symbol_leaf_name(symbol.qualified_name.as_str()),
                            file: symbol.file_path.clone(),
                        })
                        .collect::<Vec<_>>();
                    let chain = symbols
                        .iter()
                        .map(|symbol| symbol.name.clone())
                        .collect::<Vec<_>>()
                        .join(" -> ");
                    HealthCycleEntry {
                        cycle_id: (index + 1) as u32,
                        edge_count: symbols.len() as u32,
                        note: if chain.is_empty() {
                            "circular dependency detected".to_owned()
                        } else {
                            format!("Circular: {chain}")
                        },
                        symbols,
                    }
                })
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let orphans = if includes.contains(&HealthInclude::Orphans) {
            orphan_components
                .iter()
                .enumerate()
                .map(|(index, component)| HealthOrphanEntry {
                    subgraph_id: (index + 1) as u32,
                    symbols: component
                        .iter()
                        .filter_map(|symbol_id| by_symbol.get(symbol_id.as_str()))
                        .map(|symbol| HealthCycleSymbol {
                            id: symbol.id.clone(),
                            name: symbol_leaf_name(symbol.qualified_name.as_str()),
                            file: symbol.file_path.clone(),
                        })
                        .collect(),
                    note: "No inbound dependencies from the largest connected component".to_owned(),
                })
                .take(limit as usize)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let risk_hotspots = if includes.contains(&HealthInclude::RiskHotspots) {
            scored_symbols
                .iter()
                .filter(|(_, score, factors)| *score >= min_risk && factors.len() >= 2)
                .take(limit as usize)
                .map(|(context, score, factors)| HealthRiskHotspotEntry {
                    symbol_id: context.symbol.id.clone(),
                    symbol_name: symbol_leaf_name(context.symbol.qualified_name.as_str()),
                    file: context.symbol.file_path.clone(),
                    risk_score: *score,
                    risk_factors: factors.clone(),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        Ok(HealthReportResult {
            schema_version: HEALTH_SCHEMA_VERSION.to_owned(),
            analysis: HealthAnalysisSummary {
                total_symbols: by_symbol.len() as u32,
                total_edges,
                communities_detected,
                cycles_detected: cycles.len() as u32,
                orphaned_subgraphs: orphan_components.len() as u32,
                analyzed_at,
            },
            critical_symbols,
            bottlenecks,
            cycles: cycles_output,
            orphans,
            risk_hotspots,
            notes,
        })
    }
}

fn latest_drift_by_symbol(records: &[DriftResultRecord]) -> HashMap<String, f32> {
    let mut latest = HashMap::<String, (i64, f32)>::new();
    for record in records {
        let magnitude = record.drift_magnitude.unwrap_or(0.0);
        let entry = latest
            .entry(record.symbol_id.clone())
            .or_insert((record.detected_at, magnitude));
        if record.detected_at >= entry.0 {
            *entry = (record.detected_at, magnitude);
        }
    }
    latest
        .into_iter()
        .map(|(symbol_id, (_detected_at, magnitude))| (symbol_id, magnitude.clamp(0.0, 1.0)))
        .collect()
}

fn compute_risk_score(
    context: &SymbolHealthContext,
    weights: &HealthRiskWeights,
) -> (f32, Vec<String>) {
    let pagerank_contribution = weights.pagerank * context.pagerank_normalized;
    let test_gap_component = (1.0 - context.test_coverage_ratio).clamp(0.0, 1.0);
    let test_gap_contribution = weights.test_gap * test_gap_component;
    let drift_contribution = weights.drift * context.drift_magnitude;
    let no_sir_contribution = weights.no_sir * if context.has_sir { 0.0 } else { 1.0 };
    let recency_contribution = weights.recency * context.access_recency_factor;

    let mut contributions = vec![
        RiskContribution {
            label: "pagerank",
            contribution: pagerank_contribution,
            message: format!(
                "pagerank {:.2} (top {}%)",
                context.pagerank, context.pagerank_percentile
            ),
        },
        RiskContribution {
            label: "test_gap",
            contribution: test_gap_contribution,
            message: if context.edge_case_count > 0 {
                format!(
                    "only {} test guards for {} edge cases in SIR",
                    context.test_count, context.edge_case_count
                )
            } else if context.test_count == 0 {
                "no linked tests from tested_by graph".to_owned()
            } else {
                format!("{} linked tests for this symbol's file", context.test_count)
            },
        },
        RiskContribution {
            label: "drift",
            contribution: drift_contribution,
            message: format!(
                "semantic drift {:.2} over last 50 commits",
                context.drift_magnitude
            ),
        },
        RiskContribution {
            label: "no_sir",
            contribution: no_sir_contribution,
            message: "missing SIR for this symbol".to_owned(),
        },
        RiskContribution {
            label: "recency",
            contribution: recency_contribution,
            message: "recently accessed in active workflows".to_owned(),
        },
    ];

    let mut risk_score = pagerank_contribution
        + test_gap_contribution
        + drift_contribution
        + no_sir_contribution
        + recency_contribution;
    risk_score = risk_score.clamp(0.0, 1.0);

    contributions.sort_by(|left, right| {
        right
            .contribution
            .partial_cmp(&left.contribution)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.label.cmp(right.label))
    });

    let mut selected = contributions
        .iter()
        .filter(|factor| factor.contribution >= RISK_FACTOR_MIN_CONTRIBUTION)
        .map(|factor| factor.label)
        .collect::<HashSet<_>>();

    for factor in &contributions {
        if selected.len() >= 3 {
            break;
        }
        if factor.contribution <= 0.0 {
            continue;
        }
        selected.insert(factor.label);
    }

    let mut risk_factors = contributions
        .into_iter()
        .filter(|factor| selected.contains(factor.label))
        .map(|factor| factor.message)
        .collect::<Vec<_>>();

    if context.boundary_violations >= 2 {
        risk_factors.push(format!(
            "boundary violation: calls into {} other communities",
            context.boundary_violations
        ));
    }

    (risk_score, risk_factors)
}

fn parse_edge_case_count(sir_json: &str) -> u32 {
    let parsed = serde_json::from_str::<Value>(sir_json).unwrap_or(Value::Null);
    parsed
        .get("edge_cases")
        .and_then(Value::as_array)
        .or_else(|| parsed.get("error_modes").and_then(Value::as_array))
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|item| !item.trim().is_empty())
                .count() as u32
        })
        .unwrap_or(0)
}

fn recency_factor(last_accessed_at: i64) -> f32 {
    let now = now_millis();
    if last_accessed_at <= 0 || last_accessed_at > now {
        return 0.0;
    }

    let age_ms = now.saturating_sub(last_accessed_at) as f32;
    let days = age_ms / 86_400_000.0;
    (1.0 - (days / 30.0)).clamp(0.0, 1.0)
}

fn effective_includes(include: Option<&[HealthInclude]>) -> HashSet<HealthInclude> {
    let mut effective = HashSet::new();
    if let Some(include) = include {
        effective.extend(include.iter().copied());
        if !effective.is_empty() {
            return effective;
        }
    }

    effective.insert(HealthInclude::CriticalSymbols);
    effective.insert(HealthInclude::Bottlenecks);
    effective.insert(HealthInclude::Cycles);
    effective.insert(HealthInclude::Orphans);
    effective.insert(HealthInclude::RiskHotspots);
    effective
}

fn symbol_leaf_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
        .to_owned()
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use aether_core::EdgeKind;
    use aether_store::{
        CozoGraphStore, DriftResultRecord, GraphStore, IntentSnapshotRecord, ResolvedEdge, Store,
        SymbolRecord,
    };
    use tempfile::tempdir;

    use super::{
        HEALTH_SCHEMA_VERSION, HealthAnalyzer, HealthInclude, HealthReportRequest,
        SymbolHealthContext, compute_risk_score,
    };

    fn symbol(id: &str, name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: format!("demo::{name}"),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[test]
    fn health_report_finds_cycles_orphans_and_risk_hotspots() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        let cozo = CozoGraphStore::open(workspace).expect("open cozo");

        let a = symbol("sym-a", "a", "src/a.rs");
        let b = symbol("sym-b", "b", "src/b.rs");
        let c = symbol("sym-c", "c", "src/c.rs");
        let orphan = symbol("sym-orphan", "orphan", "src/legacy/orphan.rs");

        for item in [&a, &b, &c, &orphan] {
            store.upsert_symbol(item.clone()).expect("upsert symbol");
            cozo.upsert_symbol_node(item).expect("upsert cozo node");
            store
                .write_sir_blob(
                    item.id.as_str(),
                    r#"{"purpose":"demo","edge_cases":["timeout","retry"]}"#,
                )
                .expect("write sir");
        }

        for edge in [
            ResolvedEdge {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: a.file_path.clone(),
            },
            ResolvedEdge {
                source_id: b.id.clone(),
                target_id: c.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: b.file_path.clone(),
            },
            ResolvedEdge {
                source_id: c.id.clone(),
                target_id: a.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: c.file_path.clone(),
            },
        ] {
            cozo.upsert_edge(&edge).expect("upsert edge");
        }

        cozo.replace_tested_by_for_test_file(
            "tests/a_test.rs",
            &[aether_store::TestedByRecord {
                target_file: "src/a.rs".to_owned(),
                test_file: "tests/a_test.rs".to_owned(),
                intent_count: 2,
                confidence: 0.9,
                inference_method: "naming_convention".to_owned(),
            }],
        )
        .expect("replace tested_by");

        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-a".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "src/a.rs".to_owned(),
                symbol_name: "a".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.4),
                current_sir_hash: Some("hash-a2".to_owned()),
                baseline_sir_hash: Some("hash-a1".to_owned()),
                commit_range_start: Some("a".to_owned()),
                commit_range_end: Some("b".to_owned()),
                drift_summary: Some("changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: 1_700_000_000_500,
                is_acknowledged: false,
            }])
            .expect("upsert drift");

        drop(cozo);
        let analyzer = HealthAnalyzer::new(workspace).expect("new analyzer");
        let report = analyzer
            .report(HealthReportRequest {
                include: Some(vec![
                    HealthInclude::Cycles,
                    HealthInclude::Orphans,
                    HealthInclude::RiskHotspots,
                ]),
                limit: Some(10),
                min_risk: Some(0.0),
            })
            .expect("health report");

        assert_eq!(report.schema_version, HEALTH_SCHEMA_VERSION);
        assert_eq!(report.analysis.total_symbols, 4);
        assert!(!report.cycles.is_empty());
        assert!(!report.orphans.is_empty());
        assert!(!report.risk_hotspots.is_empty());
    }

    #[test]
    fn risk_score_formula_applies_weights() {
        let context = SymbolHealthContext {
            symbol: symbol("sym-a", "a", "src/a.rs"),
            pagerank: 0.5,
            pagerank_normalized: 0.5,
            pagerank_percentile: 5,
            betweenness: 0.0,
            dependents_count: 3,
            test_count: 1,
            test_coverage_ratio: 0.25,
            drift_magnitude: 0.4,
            has_sir: false,
            edge_case_count: 4,
            access_recency_factor: 0.8,
            boundary_violations: 0,
        };

        let weights = aether_config::HealthRiskWeights::default();
        let (score, factors) = compute_risk_score(&context, &weights);
        let expected = 0.3 * 0.5 + 0.25 * 0.75 + 0.2 * 0.4 + 0.15 + 0.1 * 0.8;
        assert!((score - expected).abs() < 1e-6);
        assert!(!factors.is_empty());
    }

    #[test]
    fn health_report_gracefully_handles_missing_graph_data() {
        let temp = tempdir().expect("tempdir");
        let analyzer = HealthAnalyzer::new(temp.path()).expect("new analyzer");
        let report = analyzer
            .report(HealthReportRequest::default())
            .expect("health report");
        assert_eq!(report.analysis.total_edges, 0);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("no dependency edges"))
        );
    }

    #[test]
    fn store_intent_snapshot_table_is_available_for_health_stage() {
        let temp = tempdir().expect("tempdir");
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        store
            .insert_intent_snapshot(IntentSnapshotRecord {
                snapshot_id: "snap-health-check".to_owned(),
                label: "check".to_owned(),
                scope: "file".to_owned(),
                target: "src/a.rs".to_owned(),
                symbols_json: "[]".to_owned(),
                commit_hash: None,
                created_at: 1,
            })
            .expect("insert");
        assert!(
            store
                .get_intent_snapshot("snap-health-check")
                .expect("get")
                .is_some()
        );
    }
}
