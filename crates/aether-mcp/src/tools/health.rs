use aether_config::GraphBackend;
use aether_core::{GitContext, SIR_STATUS_STALE, SymbolKind};
use aether_graph_algo::GraphAlgorithmEdge;
use aether_health::{
    FileCommunityConfig, FileSymbol, PlannerDiagnostics, ScoreReport, SemanticFileInput,
    SemanticInput, SplitSuggestion, compute_workspace_score, compute_workspace_score_filtered,
    compute_workspace_score_with_signals, detect_file_communities, format_crate_explanation,
    format_hotspots_text, suggest_split,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{SqliteStore, Store, SymbolRecord};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::{AetherMcpServer, symbol_leaf_name};
use crate::AetherMcpError;

#[derive(Debug, Clone)]
struct HealthExplainSplitOutcome {
    status: String,
    message: Option<String>,
    suggestion: Option<SplitSuggestion>,
    diagnostics: Option<PlannerDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherHealthInclude {
    CriticalSymbols,
    Bottlenecks,
    Cycles,
    Orphans,
    RiskHotspots,
}

impl From<AetherHealthInclude> for aether_analysis::HealthInclude {
    fn from(value: AetherHealthInclude) -> Self {
        match value {
            AetherHealthInclude::CriticalSymbols => aether_analysis::HealthInclude::CriticalSymbols,
            AetherHealthInclude::Bottlenecks => aether_analysis::HealthInclude::Bottlenecks,
            AetherHealthInclude::Cycles => aether_analysis::HealthInclude::Cycles,
            AetherHealthInclude::Orphans => aether_analysis::HealthInclude::Orphans,
            AetherHealthInclude::RiskHotspots => aether_analysis::HealthInclude::RiskHotspots,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthRequest {
    pub include: Option<Vec<AetherHealthInclude>>,
    pub limit: Option<u32>,
    pub min_risk: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthHotspotsRequest {
    pub limit: Option<u32>,
    pub min_score: Option<u32>,
    pub semantic: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthExplainRequest {
    pub crate_name: String,
    pub semantic: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTextResponse {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthAnalysisSummary {
    pub total_symbols: u32,
    pub total_edges: u32,
    pub communities_detected: u32,
    pub cycles_detected: u32,
    pub orphaned_subgraphs: u32,
    pub analyzed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolHealthEntry {
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBottleneckEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub betweenness: f64,
    pub pagerank: f64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthSymbolRef {
    pub id: String,
    pub name: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCycleEntry {
    pub cycle_id: u32,
    pub symbols: Vec<AetherHealthSymbolRef>,
    pub edge_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherOrphanEntry {
    pub subgraph_id: u32,
    pub symbols: Vec<AetherHealthSymbolRef>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRiskHotspotEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherHealthResponse {
    pub schema_version: String,
    pub analysis: AetherHealthAnalysisSummary,
    pub critical_symbols: Vec<AetherSymbolHealthEntry>,
    pub bottlenecks: Vec<AetherBottleneckEntry>,
    pub cycles: Vec<AetherCycleEntry>,
    pub orphans: Vec<AetherOrphanEntry>,
    pub risk_hotspots: Vec<AetherRiskHotspotEntry>,
    pub notes: Vec<String>,
}

impl AetherMcpServer {
    pub async fn aether_health_logic(
        &self,
        request: AetherHealthRequest,
    ) -> Result<AetherHealthResponse, AetherMcpError> {
        let analyzer = aether_analysis::HealthAnalyzer::new(self.workspace())?;
        let analysis_request = aether_analysis::HealthRequest {
            include: request
                .include
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            limit: request.limit.unwrap_or(10),
            min_risk: request.min_risk.unwrap_or(0.0),
        };

        let report = if self.state.config.storage.graph_backend == GraphBackend::Surreal {
            match self.state.surreal_graph_for_health().await {
                Ok(graph) => {
                    analyzer
                        .analyze_with_graph(&analysis_request, graph.as_ref())
                        .await?
                }
                Err(_) => analyzer.analyze(&analysis_request).await?,
            }
        } else {
            analyzer.analyze(&analysis_request).await?
        };

        Ok(AetherHealthResponse {
            schema_version: report.schema_version,
            analysis: AetherHealthAnalysisSummary {
                total_symbols: report.analysis.total_symbols,
                total_edges: report.analysis.total_edges,
                communities_detected: report.analysis.communities_detected,
                cycles_detected: report.analysis.cycles_detected,
                orphaned_subgraphs: report.analysis.orphaned_subgraphs,
                analyzed_at: report.analysis.analyzed_at,
            },
            critical_symbols: report
                .critical_symbols
                .into_iter()
                .map(|entry| AetherSymbolHealthEntry {
                    symbol_id: entry.symbol_id,
                    symbol_name: entry.symbol_name,
                    file: entry.file,
                    pagerank: entry.pagerank,
                    betweenness: entry.betweenness,
                    dependents_count: entry.dependents_count,
                    has_sir: entry.has_sir,
                    test_count: entry.test_count,
                    drift_magnitude: entry.drift_magnitude,
                    risk_score: entry.risk_score,
                    risk_factors: entry.risk_factors,
                })
                .collect(),
            bottlenecks: report
                .bottlenecks
                .into_iter()
                .map(|entry| AetherBottleneckEntry {
                    symbol_id: entry.symbol_id,
                    symbol_name: entry.symbol_name,
                    file: entry.file,
                    betweenness: entry.betweenness,
                    pagerank: entry.pagerank,
                    note: entry.note,
                })
                .collect(),
            cycles: report
                .cycles
                .into_iter()
                .map(|entry| AetherCycleEntry {
                    cycle_id: entry.cycle_id,
                    symbols: entry
                        .symbols
                        .into_iter()
                        .map(|symbol| AetherHealthSymbolRef {
                            id: symbol.id,
                            name: symbol.name,
                            file: symbol.file,
                        })
                        .collect(),
                    edge_count: entry.edge_count,
                    note: entry.note,
                })
                .collect(),
            orphans: report
                .orphans
                .into_iter()
                .map(|entry| AetherOrphanEntry {
                    subgraph_id: entry.subgraph_id,
                    symbols: entry
                        .symbols
                        .into_iter()
                        .map(|symbol| AetherHealthSymbolRef {
                            id: symbol.id,
                            name: symbol.name,
                            file: symbol.file,
                        })
                        .collect(),
                    note: entry.note,
                })
                .collect(),
            risk_hotspots: report
                .risk_hotspots
                .into_iter()
                .map(|entry| AetherRiskHotspotEntry {
                    symbol_id: entry.symbol_id,
                    symbol_name: entry.symbol_name,
                    file: entry.file,
                    risk_score: entry.risk_score,
                    risk_factors: entry.risk_factors,
                })
                .collect(),
            notes: report.notes,
        })
    }

    pub async fn aether_health_hotspots_logic(
        &self,
        request: AetherHealthHotspotsRequest,
    ) -> Result<String, AetherMcpError> {
        let limit = request.limit.unwrap_or(5).clamp(1, 100) as usize;
        let min_score = request.min_score.unwrap_or(25).min(100);
        let semantic = request.semantic.unwrap_or(true);
        let report = self.compute_health_score_report(semantic, &[]).await?;
        Ok(format_hotspots_text(&report, limit, min_score))
    }

    pub async fn aether_health_explain_logic(
        &self,
        request: AetherHealthExplainRequest,
    ) -> Result<String, AetherMcpError> {
        let semantic = request.semantic.unwrap_or(true);
        let report = self.compute_health_score_report(semantic, &[]).await?;
        let crate_name = request.crate_name.trim();
        let Some(crate_score) = report
            .crates
            .iter()
            .find(|crate_score| crate_score.name == crate_name)
        else {
            let available = report
                .crates
                .iter()
                .map(|crate_score| crate_score.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AetherMcpError::Message(format!(
                "unknown crate '{crate_name}'. Available crates: {available}"
            )));
        };

        let split_outcome = if crate_score.score >= 50 {
            if semantic {
                self.split_suggestion_for_crate(crate_score).await
            } else {
                HealthExplainSplitOutcome {
                    status: "skipped".to_owned(),
                    message: Some("split suggestion skipped because semantic=false".to_owned()),
                    suggestion: None,
                    diagnostics: None,
                }
            }
        } else {
            HealthExplainSplitOutcome {
                status: "not_applicable".to_owned(),
                message: None,
                suggestion: None,
                diagnostics: None,
            }
        };

        let mut rendered = format_crate_explanation(crate_score, split_outcome.suggestion.as_ref());
        if crate_score.score >= 50 {
            append_split_outcome_text(&mut rendered, &split_outcome);
        }
        Ok(rendered)
    }

    async fn compute_health_score_report(
        &self,
        semantic: bool,
        crate_filter: &[String],
    ) -> Result<ScoreReport, AetherMcpError> {
        if semantic {
            let git = GitContext::open(self.workspace());
            let semantic_input = match self.load_health_semantic_input().await {
                Ok(input) => input,
                Err(err) => {
                    tracing::warn!("mcp health semantic bridge unavailable: {err}");
                    None
                }
            };
            compute_workspace_score_with_signals(
                self.workspace(),
                &self.state.config.health_score,
                crate_filter,
                git.as_ref(),
                semantic_input.as_ref(),
            )
            .map_err(|err| {
                AetherMcpError::Message(format!("failed to compute health score: {err}"))
            })
        } else if crate_filter.is_empty() {
            compute_workspace_score(self.workspace(), &self.state.config.health_score).map_err(
                |err| AetherMcpError::Message(format!("failed to compute health score: {err}")),
            )
        } else {
            compute_workspace_score_filtered(
                self.workspace(),
                &self.state.config.health_score,
                crate_filter,
            )
            .map_err(|err| {
                AetherMcpError::Message(format!("failed to compute health score: {err}"))
            })
        }
    }

    async fn load_health_semantic_input(&self) -> Result<Option<SemanticInput>, AetherMcpError> {
        let analyzer = aether_analysis::HealthAnalyzer::new(self.workspace())?;
        let centrality = analyzer.centrality_by_file().await?;
        if centrality.files.is_empty() && !centrality.notes.is_empty() {
            return Ok(None);
        }

        let drift_by_symbol = latest_semantic_drift_by_symbol(self.state.store.as_ref())?;
        let community_by_symbol = self
            .state
            .store
            .list_latest_community_snapshot()?
            .into_iter()
            .map(|entry| (entry.symbol_id, entry.community_id))
            .collect::<HashMap<_, _>>();

        let mut files = HashMap::new();
        for entry in centrality.files {
            let path = aether_core::normalize_path(entry.file.as_str());
            let symbols = self.state.store.list_symbols_for_file(path.as_str())?;
            if symbols.is_empty() {
                continue;
            }

            let drifted_symbol_count = symbols
                .iter()
                .filter(|symbol| {
                    drift_by_symbol
                        .get(symbol.id.as_str())
                        .is_some_and(|magnitude| *magnitude > 0.3)
                })
                .count();
            let stale_or_missing_sir_count = symbols
                .iter()
                .filter(|symbol| {
                    self.state
                        .store
                        .get_sir_meta(symbol.id.as_str())
                        .ok()
                        .flatten()
                        .is_none_or(|meta| {
                            meta.sir_status
                                .trim()
                                .eq_ignore_ascii_case(SIR_STATUS_STALE)
                        })
                })
                .count();
            let community_count = symbols
                .iter()
                .filter_map(|symbol| community_by_symbol.get(symbol.id.as_str()).copied())
                .collect::<HashSet<_>>()
                .len();
            let has_test_coverage = symbols.iter().any(|symbol| {
                self.state
                    .store
                    .list_test_intents_for_symbol(symbol.id.as_str())
                    .map(|records| !records.is_empty())
                    .unwrap_or(false)
            });

            files.insert(
                path,
                SemanticFileInput {
                    max_pagerank: entry.max_pagerank,
                    symbol_count: symbols.len(),
                    drifted_symbol_count,
                    stale_or_missing_sir_count,
                    community_count,
                    has_test_coverage,
                },
            );
        }

        if files.is_empty() {
            return Ok(None);
        }

        Ok(Some(SemanticInput {
            workspace_max_pagerank: centrality.workspace_max_pagerank,
            files,
        }))
    }

    async fn split_suggestion_for_crate(
        &self,
        crate_score: &aether_health::CrateScore,
    ) -> HealthExplainSplitOutcome {
        let Some(file_path) = crate_score.metrics.max_file_path.as_deref() else {
            return HealthExplainSplitOutcome {
                status: "no_split".to_owned(),
                message: Some("hotspot file path is unavailable".to_owned()),
                suggestion: None,
                diagnostics: None,
            };
        };
        if self.state.config.storage.graph_backend != GraphBackend::Surreal {
            return HealthExplainSplitOutcome {
                status: "unavailable".to_owned(),
                message: Some(
                    "split suggestions require storage.graph_backend = \"surreal\"".to_owned(),
                ),
                suggestion: None,
                diagnostics: None,
            };
        }

        let loaded = match load_embedding_provider_from_config(
            self.workspace(),
            EmbeddingProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some("embeddings are disabled for this workspace".to_owned()),
                    suggestion: None,
                    diagnostics: None,
                };
            }
            Err(err) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some(format!("failed to load embedding provider: {err}")),
                    suggestion: None,
                    diagnostics: None,
                };
            }
        };

        let Some(vector_store) = self.state.vector_store.as_ref().map(std::sync::Arc::clone) else {
            return HealthExplainSplitOutcome {
                status: "unavailable".to_owned(),
                message: Some("vector store is unavailable".to_owned()),
                suggestion: None,
                diagnostics: None,
            };
        };

        let graph = match self.state.surreal_graph_for_health().await {
            Ok(graph) => graph,
            Err(err) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some(err.to_string()),
                    suggestion: None,
                    diagnostics: None,
                };
            }
        };

        let all_edges = match graph.list_dependency_edges().await {
            Ok(edges) => edges
                .into_iter()
                .map(|edge| GraphAlgorithmEdge {
                    source_id: edge.source_symbol_id,
                    target_id: edge.target_symbol_id,
                    edge_kind: edge.edge_kind,
                })
                .collect::<Vec<_>>(),
            Err(err) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some(format!("failed to load dependency edges: {err}")),
                    suggestion: None,
                    diagnostics: None,
                };
            }
        };

        let symbol_records = match self.state.store.list_symbols_for_file(file_path) {
            Ok(symbols) => symbols,
            Err(err) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some(format!("failed to load symbols for {file_path}: {err}")),
                    suggestion: None,
                    diagnostics: None,
                };
            }
        };
        if symbol_records.is_empty() {
            return HealthExplainSplitOutcome {
                status: "unavailable".to_owned(),
                message: Some(format!("no indexed symbols were found for {file_path}")),
                suggestion: None,
                diagnostics: None,
            };
        }

        let symbol_ids = symbol_records
            .iter()
            .map(|symbol| symbol.id.clone())
            .collect::<Vec<_>>();
        let symbol_id_set = symbol_ids
            .iter()
            .map(|symbol_id| symbol_id.as_str())
            .collect::<HashSet<_>>();
        let structural_edges = all_edges
            .iter()
            .filter(|edge| {
                symbol_id_set.contains(edge.source_id.as_str())
                    && symbol_id_set.contains(edge.target_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        let embedding_records = match vector_store
            .list_embeddings_for_symbols(
                &loaded.provider_name,
                &loaded.model_name,
                symbol_ids.as_slice(),
            )
            .await
        {
            Ok(records) => records,
            Err(err) => {
                return HealthExplainSplitOutcome {
                    status: "unavailable".to_owned(),
                    message: Some(format!("failed to load embeddings for {file_path}: {err}")),
                    suggestion: None,
                    diagnostics: None,
                };
            }
        };
        let embedding_by_id = embedding_records
            .into_iter()
            .map(|record| (record.symbol_id, record.embedding))
            .collect::<HashMap<_, _>>();
        let file_symbols = symbol_records
            .iter()
            .map(|record| self.build_planner_file_symbol(record, &embedding_by_id))
            .collect::<Vec<_>>();
        let planner_config = FileCommunityConfig {
            semantic_rescue_threshold: self.state.config.planner.semantic_rescue_threshold,
            semantic_rescue_max_k: self.state.config.planner.semantic_rescue_max_k,
            community_resolution: self.state.config.planner.community_resolution,
            min_community_size: self.state.config.planner.min_community_size,
        };

        let (assignments, diagnostics) = detect_file_communities(
            structural_edges.as_slice(),
            file_symbols.as_slice(),
            &planner_config,
        );
        if assignments.is_empty() {
            return HealthExplainSplitOutcome {
                status: "no_split".to_owned(),
                message: Some("all non-test symbols were loners after rescue passes".to_owned()),
                suggestion: None,
                diagnostics: Some(diagnostics),
            };
        }

        let community_count = assignments
            .iter()
            .map(|(_, community_id)| *community_id)
            .collect::<HashSet<_>>()
            .len();
        if community_count < 2 {
            return HealthExplainSplitOutcome {
                status: "no_split".to_owned(),
                message: Some("only one actionable community was detected".to_owned()),
                suggestion: None,
                diagnostics: Some(diagnostics),
            };
        }

        match suggest_split(
            file_path,
            crate_score.score,
            structural_edges.as_slice(),
            file_symbols.as_slice(),
            &planner_config,
        ) {
            Some((suggestion, diagnostics)) => HealthExplainSplitOutcome {
                status: "suggested".to_owned(),
                message: None,
                suggestion: Some(suggestion),
                diagnostics: Some(diagnostics),
            },
            None => HealthExplainSplitOutcome {
                status: "no_split".to_owned(),
                message: Some("no actionable split suggestion was produced".to_owned()),
                suggestion: None,
                diagnostics: Some(diagnostics),
            },
        }
    }

    fn build_planner_file_symbol(
        &self,
        record: &SymbolRecord,
        embedding_by_id: &HashMap<String, Vec<f32>>,
    ) -> FileSymbol {
        FileSymbol {
            symbol_id: record.id.clone(),
            name: symbol_leaf_name(record.qualified_name.as_str()).to_owned(),
            qualified_name: record.qualified_name.clone(),
            kind: parse_symbol_kind(record.kind.as_str()),
            is_test: self.symbol_is_test(record),
            embedding: embedding_by_id.get(record.id.as_str()).cloned(),
        }
    }

    fn symbol_is_test(&self, record: &SymbolRecord) -> bool {
        if self
            .state
            .store
            .list_test_intents_for_symbol(record.id.as_str())
            .map(|records| !records.is_empty())
            .unwrap_or(false)
        {
            return true;
        }

        let leaf_name = symbol_leaf_name(record.qualified_name.as_str()).to_ascii_lowercase();
        if leaf_name.starts_with("test_") {
            return true;
        }

        let normalized_path =
            aether_core::normalize_path(record.file_path.as_str()).to_ascii_lowercase();
        normalized_path.starts_with("tests/") || normalized_path.contains("/tests/")
    }
}

fn latest_semantic_drift_by_symbol(
    store: &SqliteStore,
) -> Result<HashMap<String, f64>, AetherMcpError> {
    let mut drift_by_symbol = HashMap::new();
    for record in store.list_drift_results(true)? {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };
        drift_by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(drift_by_symbol)
}

fn append_split_outcome_text(rendered: &mut String, outcome: &HealthExplainSplitOutcome) {
    match outcome.status.as_str() {
        "suggested" => {
            if let Some(suggestion) = outcome.suggestion.as_ref() {
                rendered.push_str("\n\nSuggested module files:");
                for module in &suggestion.suggested_modules {
                    rendered.push_str(&format!(
                        "\n  - {} -> {}",
                        module.name, module.suggested_file_path
                    ));
                }
            }
            if let Some(diagnostics) = outcome.diagnostics.as_ref() {
                rendered.push_str("\n\nSplit diagnostics:");
                append_planner_diagnostics_text(rendered, diagnostics);
            }
        }
        "not_applicable" => {}
        _ => {
            rendered.push_str("\n\nSplit suggestion:");
            rendered.push_str(&format!("\n  status: {}", outcome.status));
            if let Some(message) = outcome.message.as_ref() {
                rendered.push_str(&format!("\n  message: {message}"));
            }
            if let Some(diagnostics) = outcome.diagnostics.as_ref() {
                rendered.push_str("\n\nSplit diagnostics:");
                append_planner_diagnostics_text(rendered, diagnostics);
            }
        }
    }
}

fn append_planner_diagnostics_text(rendered: &mut String, diagnostics: &PlannerDiagnostics) {
    rendered.push_str(&format!(
        "\n  confidence: {} ({:.2})",
        diagnostics.confidence_label, diagnostics.confidence
    ));
    rendered.push_str(&format!(
        "\n  stability: {:.2}",
        diagnostics.stability_score
    ));
    rendered.push_str(&format!("\n  symbols_total: {}", diagnostics.symbols_total));
    rendered.push_str(&format!(
        "\n  symbols_filtered_test: {}",
        diagnostics.symbols_filtered_test
    ));
    rendered.push_str(&format!(
        "\n  symbols_anchored_type: {}",
        diagnostics.symbols_anchored_type
    ));
    rendered.push_str(&format!(
        "\n  symbols_rescued_container: {}",
        diagnostics.symbols_rescued_container
    ));
    rendered.push_str(&format!(
        "\n  symbols_rescued_semantic: {}",
        diagnostics.symbols_rescued_semantic
    ));
    rendered.push_str(&format!("\n  symbols_loner: {}", diagnostics.symbols_loner));
    rendered.push_str(&format!(
        "\n  communities_before_merge: {}",
        diagnostics.communities_before_merge
    ));
    rendered.push_str(&format!(
        "\n  communities_after_merge: {}",
        diagnostics.communities_after_merge
    ));
    rendered.push_str(&format!(
        "\n  embedding_coverage_pct: {:.2}",
        diagnostics.embedding_coverage_pct
    ));
}

fn parse_symbol_kind(raw: &str) -> SymbolKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "variable" => SymbolKind::Variable,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "interface" => SymbolKind::Interface,
        "type_alias" => SymbolKind::TypeAlias,
        _ => SymbolKind::Function,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::AetherHealthExplainRequest;
    use crate::AetherMcpServer;
    use aether_core::EdgeKind;
    use aether_store::{
        GraphStore, ResolvedEdge, SqliteStore, Store, SurrealGraphStore, SymbolEmbeddingRecord,
        SymbolRecord,
    };
    use tempfile::tempdir;

    fn write_health_explain_config(workspace: &Path, graph_backend: &str) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[storage]
graph_backend = "{graph_backend}"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"

[health_score]
file_loc_warn = 1
file_loc_fail = 2
trait_method_warn = 1
trait_method_fail = 2
"#
            ),
        )
        .expect("write config");
    }

    fn symbol_record(id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    fn embedding_record(symbol_id: &str, embedding: Vec<f32>) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: format!("sir-{symbol_id}"),
            provider: "qwen3_local".to_owned(),
            model: "qwen3-embeddings-4B".to_owned(),
            embedding,
            updated_at: 1_700_000_000_000,
        }
    }

    #[tokio::test]
    async fn aether_health_explain_includes_split_diagnostics() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nresolver = \"2\"\n",
        )
        .expect("write workspace Cargo.toml");
        fs::create_dir_all(workspace.join("crates/example/src")).expect("create src dir");
        fs::write(
            workspace.join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write crate Cargo.toml");
        fs::write(
            workspace.join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n\npub fn sir_alpha() -> i32 { 1 }\npub fn sir_beta() -> i32 { sir_alpha() }\npub fn sir_gamma() -> i32 { sir_beta() }\npub fn sir_delta() -> i32 { sir_gamma() }\npub fn note_alpha() -> i32 { 2 }\npub fn note_beta() -> i32 { note_alpha() }\npub fn note_gamma() -> i32 { note_beta() }\npub fn note_delta() -> i32 { note_gamma() }\n",
        )
        .expect("write lib.rs");
        write_health_explain_config(workspace, "surreal");

        let server = AetherMcpServer::init(workspace, false)
            .await
            .expect("init mcp server");
        let store = SqliteStore::open(workspace).expect("open store");
        let symbols = vec![
            symbol_record("sym-sir-a", "crate::sir_alpha", "crates/example/src/lib.rs"),
            symbol_record("sym-sir-b", "crate::sir_beta", "crates/example/src/lib.rs"),
            symbol_record("sym-sir-c", "crate::sir_gamma", "crates/example/src/lib.rs"),
            symbol_record("sym-sir-d", "crate::sir_delta", "crates/example/src/lib.rs"),
            symbol_record(
                "sym-note-a",
                "crate::note_alpha",
                "crates/example/src/lib.rs",
            ),
            symbol_record(
                "sym-note-b",
                "crate::note_beta",
                "crates/example/src/lib.rs",
            ),
            symbol_record(
                "sym-note-c",
                "crate::note_gamma",
                "crates/example/src/lib.rs",
            ),
            symbol_record(
                "sym-note-d",
                "crate::note_delta",
                "crates/example/src/lib.rs",
            ),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-a", vec![1.0, 0.0]))
            .expect("seed sir-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-b", vec![0.95, 0.05]))
            .expect("seed sir-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-c", vec![0.92, 0.08]))
            .expect("seed sir-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-d", vec![0.9, 0.1]))
            .expect("seed sir-d embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-a", vec![0.0, 1.0]))
            .expect("seed note-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-b", vec![0.05, 0.95]))
            .expect("seed note-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-c", vec![0.08, 0.92]))
            .expect("seed note-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-d", vec![0.1, 0.9]))
            .expect("seed note-d embedding");

        let graph = SurrealGraphStore::open(workspace)
            .await
            .expect("open surreal graph");
        for symbol in &symbols {
            graph
                .upsert_symbol_node(symbol)
                .await
                .expect("upsert surreal symbol");
        }
        for (source_id, target_id) in [
            ("sym-sir-a", "sym-sir-b"),
            ("sym-sir-b", "sym-sir-c"),
            ("sym-sir-c", "sym-sir-d"),
            ("sym-note-a", "sym-note-b"),
            ("sym-note-b", "sym-note-c"),
            ("sym-note-c", "sym-note-d"),
        ] {
            graph
                .upsert_edge(&ResolvedEdge {
                    source_id: source_id.to_owned(),
                    target_id: target_id.to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "crates/example/src/lib.rs".to_owned(),
                })
                .await
                .expect("upsert surreal edge");
        }

        let rendered = server
            .aether_health_explain_logic(AetherHealthExplainRequest {
                crate_name: "example".to_owned(),
                semantic: Some(true),
            })
            .await
            .expect("health explain");

        assert!(rendered.contains("Split suggestion:"));
        assert!(rendered.contains("sir_ops"));
        assert!(rendered.contains("note_ops"));
        assert!(rendered.contains("Split diagnostics:"));
        assert!(rendered.contains("stability:"));
        assert!(rendered.contains("Suggested module files:"));
    }

    #[tokio::test]
    async fn aether_health_explain_reports_unavailable_on_non_surreal_backend() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::write(
            workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nresolver = \"2\"\n",
        )
        .expect("write workspace Cargo.toml");
        fs::create_dir_all(workspace.join("crates/example/src")).expect("create src dir");
        fs::write(
            workspace.join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write crate Cargo.toml");
        fs::write(
            workspace.join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n",
        )
        .expect("write lib.rs");
        write_health_explain_config(workspace, "sqlite");

        let server = AetherMcpServer::init(workspace, false)
            .await
            .expect("init mcp server");

        let rendered = server
            .aether_health_explain_logic(AetherHealthExplainRequest {
                crate_name: "example".to_owned(),
                semantic: Some(true),
            })
            .await
            .expect("health explain");

        assert!(rendered.contains("status: unavailable"));
        assert!(rendered.contains("storage.graph_backend = \"surreal\""));
    }
}
