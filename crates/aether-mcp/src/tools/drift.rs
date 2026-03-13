use aether_analysis::{
    AcknowledgeDriftRequest as AnalysisAcknowledgeDriftRequest, CausalAnalyzer, DriftAnalyzer,
    DriftInclude as AnalysisDriftInclude, DriftReportRequest as AnalysisDriftReportRequest,
    TraceCauseRequest as AnalysisTraceCauseRequest,
};
use aether_store::{SqliteStore, Store, SymbolRecord};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, symbol_leaf_name};
use crate::AetherMcpError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherDriftInclude {
    Semantic,
    Boundary,
    Structural,
}

impl From<AetherDriftInclude> for AnalysisDriftInclude {
    fn from(value: AetherDriftInclude) -> Self {
        match value {
            AetherDriftInclude::Semantic => AnalysisDriftInclude::Semantic,
            AetherDriftInclude::Boundary => AnalysisDriftInclude::Boundary,
            AetherDriftInclude::Structural => AnalysisDriftInclude::Structural,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDriftReportRequest {
    pub window: Option<String>,
    pub include: Option<Vec<AetherDriftInclude>>,
    pub min_drift_magnitude: Option<f32>,
    pub include_acknowledged: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDriftReportWindow {
    pub from_commit: String,
    pub to_commit: String,
    pub commit_count: u32,
    pub analyzed_at: i64,
    pub limited_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDriftSummary {
    pub symbols_analyzed: u32,
    pub semantic_drifts: u32,
    pub boundary_violations: u32,
    pub emerging_hubs: u32,
    pub new_cycles: u32,
    pub orphaned_subgraphs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDriftTestCoverage {
    pub has_tests: bool,
    pub test_count: u32,
    pub intents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSemanticDriftEntry {
    pub result_id: String,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub drift_magnitude: f32,
    pub similarity: f32,
    pub drift_summary: String,
    pub commit_range: [String; 2],
    pub test_coverage: AetherDriftTestCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBoundaryViolationEntry {
    pub result_id: String,
    pub source_symbol: String,
    pub source_file: String,
    pub source_community: i64,
    pub target_symbol: String,
    pub target_file: String,
    pub target_community: i64,
    pub edge_type: String,
    pub first_seen_commit: String,
    pub informational: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherEmergingHubEntry {
    pub result_id: String,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub current_pagerank: f32,
    pub previous_pagerank: f32,
    pub dependents_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherNewCycleEntry {
    pub result_id: String,
    pub symbols: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherOrphanedSubgraphEntry {
    pub result_id: String,
    pub symbols: Vec<String>,
    pub files: Vec<String>,
    pub total_symbols: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct AetherStructuralAnomalies {
    pub emerging_hubs: Vec<AetherEmergingHubEntry>,
    pub new_cycles: Vec<AetherNewCycleEntry>,
    pub orphaned_subgraphs: Vec<AetherOrphanedSubgraphEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDriftReportResponse {
    pub schema_version: String,
    pub analysis_window: AetherDriftReportWindow,
    pub summary: AetherDriftSummary,
    pub semantic_drift: Vec<AetherSemanticDriftEntry>,
    pub boundary_violations: Vec<AetherBoundaryViolationEntry>,
    pub structural_anomalies: AetherStructuralAnomalies,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAcknowledgeDriftRequest {
    pub result_ids: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAcknowledgeDriftResponse {
    pub schema_version: String,
    pub acknowledged: u32,
    pub note_created: bool,
    pub note_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseRequest {
    pub symbol: Option<String>,
    pub symbol_id: Option<String>,
    pub file: Option<String>,
    pub lookback: Option<String>,
    pub max_depth: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseTarget {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseAnalysisWindow {
    pub lookback: String,
    pub max_depth: u32,
    pub upstream_symbols_scanned: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseSirDiff {
    pub purpose_changed: bool,
    pub purpose_before: String,
    pub purpose_after: String,
    pub edge_cases_added: Vec<String>,
    pub edge_cases_removed: Vec<String>,
    pub dependencies_added: Vec<String>,
    pub dependencies_removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseChange {
    pub commit: String,
    pub author: String,
    pub date: String,
    pub change_magnitude: f32,
    pub sir_diff: AetherTraceCauseSirDiff,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseCoupling {
    pub fused_score: f32,
    pub coupling_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseEntry {
    pub rank: u32,
    pub causal_score: f32,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub dependency_path: Vec<String>,
    pub depth: u32,
    pub change: AetherTraceCauseChange,
    pub coupling: AetherTraceCauseCoupling,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraceCauseResponse {
    pub schema_version: String,
    pub target: AetherTraceCauseTarget,
    pub analysis_window: AetherTraceCauseAnalysisWindow,
    pub causal_chain: Vec<AetherTraceCauseEntry>,
    pub no_change_upstream: u32,
    pub skipped_missing_history: u32,
    pub embedding_fallback_count: u32,
    pub notes: Vec<String>,
}

impl AetherMcpServer {
    pub fn aether_drift_report_logic(
        &self,
        request: AetherDriftReportRequest,
    ) -> Result<AetherDriftReportResponse, AetherMcpError> {
        let analyzer = DriftAnalyzer::new(self.workspace())?;
        let report = analyzer.report(AnalysisDriftReportRequest {
            window: request.window,
            include: request
                .include
                .map(|items| items.into_iter().map(Into::into).collect()),
            min_drift_magnitude: request.min_drift_magnitude,
            include_acknowledged: request.include_acknowledged,
        })?;

        Ok(AetherDriftReportResponse {
            schema_version: report.schema_version,
            analysis_window: AetherDriftReportWindow {
                from_commit: report.analysis_window.from_commit,
                to_commit: report.analysis_window.to_commit,
                commit_count: report.analysis_window.commit_count,
                analyzed_at: report.analysis_window.analyzed_at,
                limited_history: report.analysis_window.limited_history,
            },
            summary: AetherDriftSummary {
                symbols_analyzed: report.summary.symbols_analyzed,
                semantic_drifts: report.summary.semantic_drifts,
                boundary_violations: report.summary.boundary_violations,
                emerging_hubs: report.summary.emerging_hubs,
                new_cycles: report.summary.new_cycles,
                orphaned_subgraphs: report.summary.orphaned_subgraphs,
            },
            semantic_drift: report
                .semantic_drift
                .into_iter()
                .map(|entry| AetherSemanticDriftEntry {
                    result_id: entry.result_id,
                    symbol_id: entry.symbol_id,
                    symbol_name: entry.symbol_name,
                    file: entry.file,
                    drift_magnitude: entry.drift_magnitude,
                    similarity: entry.similarity,
                    drift_summary: entry.drift_summary,
                    commit_range: entry.commit_range,
                    test_coverage: AetherDriftTestCoverage {
                        has_tests: entry.test_coverage.has_tests,
                        test_count: entry.test_coverage.test_count,
                        intents: entry.test_coverage.intents,
                    },
                })
                .collect(),
            boundary_violations: report
                .boundary_violations
                .into_iter()
                .map(|entry| AetherBoundaryViolationEntry {
                    result_id: entry.result_id,
                    source_symbol: entry.source_symbol,
                    source_file: entry.source_file,
                    source_community: entry.source_community,
                    target_symbol: entry.target_symbol,
                    target_file: entry.target_file,
                    target_community: entry.target_community,
                    edge_type: entry.edge_type,
                    first_seen_commit: entry.first_seen_commit,
                    informational: entry.informational,
                    note: entry.note,
                })
                .collect(),
            structural_anomalies: AetherStructuralAnomalies {
                emerging_hubs: report
                    .structural_anomalies
                    .emerging_hubs
                    .into_iter()
                    .map(|entry| AetherEmergingHubEntry {
                        result_id: entry.result_id,
                        symbol_id: entry.symbol_id,
                        symbol_name: entry.symbol_name,
                        file: entry.file,
                        current_pagerank: entry.current_pagerank,
                        previous_pagerank: entry.previous_pagerank,
                        dependents_count: entry.dependents_count,
                        note: entry.note,
                    })
                    .collect(),
                new_cycles: report
                    .structural_anomalies
                    .new_cycles
                    .into_iter()
                    .map(|entry| AetherNewCycleEntry {
                        result_id: entry.result_id,
                        symbols: entry.symbols,
                        note: entry.note,
                    })
                    .collect(),
                orphaned_subgraphs: report
                    .structural_anomalies
                    .orphaned_subgraphs
                    .into_iter()
                    .map(|entry| AetherOrphanedSubgraphEntry {
                        result_id: entry.result_id,
                        symbols: entry.symbols,
                        files: entry.files,
                        total_symbols: entry.total_symbols,
                        note: entry.note,
                    })
                    .collect(),
            },
        })
    }

    pub fn aether_trace_cause_logic(
        &self,
        request: AetherTraceCauseRequest,
    ) -> Result<AetherTraceCauseResponse, AetherMcpError> {
        let store = self.state.store.as_ref();
        let target_symbol = self.resolve_trace_cause_symbol(store, &request)?;
        let analyzer = CausalAnalyzer::new(self.workspace())?;
        let result = analyzer.trace_cause(AnalysisTraceCauseRequest {
            target_symbol_id: target_symbol.id,
            lookback: request.lookback,
            max_depth: request.max_depth,
            limit: request.limit,
        })?;

        Ok(AetherTraceCauseResponse {
            schema_version: result.schema_version,
            target: AetherTraceCauseTarget {
                symbol_id: result.target.symbol_id,
                symbol_name: result.target.symbol_name,
                file: result.target.file,
            },
            analysis_window: AetherTraceCauseAnalysisWindow {
                lookback: result.analysis_window.lookback,
                max_depth: result.analysis_window.max_depth,
                upstream_symbols_scanned: result.analysis_window.upstream_symbols_scanned,
            },
            causal_chain: result
                .causal_chain
                .into_iter()
                .map(|entry| AetherTraceCauseEntry {
                    rank: entry.rank,
                    causal_score: entry.causal_score,
                    symbol_id: entry.symbol_id,
                    symbol_name: entry.symbol_name,
                    file: entry.file,
                    dependency_path: entry.dependency_path,
                    depth: entry.depth,
                    change: AetherTraceCauseChange {
                        commit: entry.change.commit,
                        author: entry.change.author,
                        date: entry.change.date,
                        change_magnitude: entry.change.change_magnitude,
                        sir_diff: AetherTraceCauseSirDiff {
                            purpose_changed: entry.change.sir_diff.purpose_changed,
                            purpose_before: entry.change.sir_diff.purpose_before,
                            purpose_after: entry.change.sir_diff.purpose_after,
                            edge_cases_added: entry.change.sir_diff.edge_cases_added,
                            edge_cases_removed: entry.change.sir_diff.edge_cases_removed,
                            dependencies_added: entry.change.sir_diff.dependencies_added,
                            dependencies_removed: entry.change.sir_diff.dependencies_removed,
                        },
                    },
                    coupling: AetherTraceCauseCoupling {
                        fused_score: entry.coupling.fused_score,
                        coupling_type: entry.coupling.coupling_type,
                    },
                })
                .collect(),
            no_change_upstream: result.no_change_upstream,
            skipped_missing_history: result.skipped_missing_history,
            embedding_fallback_count: result.embedding_fallback_count,
            notes: result.notes,
        })
    }

    pub fn aether_acknowledge_drift_logic(
        &self,
        request: AetherAcknowledgeDriftRequest,
    ) -> Result<AetherAcknowledgeDriftResponse, AetherMcpError> {
        self.state.require_writable()?;
        let analyzer = DriftAnalyzer::new(self.workspace())?;
        let result = analyzer.acknowledge_drift(AnalysisAcknowledgeDriftRequest {
            result_ids: request.result_ids,
            note: request.note,
        })?;
        Ok(AetherAcknowledgeDriftResponse {
            schema_version: result.schema_version,
            acknowledged: result.acknowledged,
            note_created: result.note_created,
            note_id: result.note_id,
        })
    }

    fn resolve_trace_cause_symbol(
        &self,
        store: &SqliteStore,
        request: &AetherTraceCauseRequest,
    ) -> Result<SymbolRecord, AetherMcpError> {
        let symbol_not_found =
            || AetherMcpError::Message("symbol not found, try aether_search to find it".to_owned());
        if let Some(symbol_id) = request.symbol_id.as_deref().map(str::trim)
            && !symbol_id.is_empty()
        {
            return store
                .get_symbol_record(symbol_id)?
                .ok_or_else(symbol_not_found);
        }

        let symbol = request.symbol.as_deref().map(str::trim).unwrap_or_default();
        let file = request
            .file
            .as_deref()
            .map(aether_core::normalize_path)
            .unwrap_or_default();
        if symbol.is_empty() || file.is_empty() {
            return Err(AetherMcpError::Message(
                "provide either symbol_id, or symbol + file".to_owned(),
            ));
        }

        let mut matches = store
            .list_symbols_for_file(file.as_str())?
            .into_iter()
            .filter(|candidate| {
                candidate.qualified_name == symbol
                    || symbol_leaf_name(candidate.qualified_name.as_str()) == symbol
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| left.id.cmp(&right.id));
        matches.into_iter().next().ok_or_else(symbol_not_found)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::EdgeKind;
    use aether_store::{DriftResultRecord, ResolvedEdge, SqliteStore, Store, SymbolRecord};
    use tempfile::tempdir;

    use super::{AetherAcknowledgeDriftRequest, AetherDriftReportRequest, AetherTraceCauseRequest};
    use crate::AetherMcpServer;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    #[test]
    fn aether_drift_report_logic_returns_schema() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "sqlite"

[drift]
enabled = true
drift_threshold = 0.85
analysis_window = "10 commits"
auto_analyze = false
hub_percentile = 95
"#,
        )
        .expect("write config");

        let server = AetherMcpServer::new(workspace, false).expect("new mcp server");
        let response = server
            .aether_drift_report_logic(AetherDriftReportRequest {
                window: Some("1 commits".to_owned()),
                include: None,
                min_drift_magnitude: Some(0.0),
                include_acknowledged: Some(false),
            })
            .expect("drift report");
        assert_eq!(response.schema_version, "1.0");
    }

    #[test]
    fn aether_acknowledge_drift_logic_marks_rows() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let server = AetherMcpServer::new(workspace, false).expect("new mcp server");
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-mcp-1".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "src/a.rs".to_owned(),
                symbol_name: "a".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.5),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some(String::new()),
                commit_range_end: Some(String::new()),
                drift_summary: Some("changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: 1_700_000_000_000,
                is_acknowledged: false,
            }])
            .expect("seed drift row");

        let response = server
            .aether_acknowledge_drift_logic(AetherAcknowledgeDriftRequest {
                result_ids: vec!["drift-mcp-1".to_owned()],
                note: "intentional".to_owned(),
            })
            .expect("ack drift");
        assert_eq!(response.acknowledged, 1);

        let rows = store
            .list_drift_results_by_ids(&["drift-mcp-1".to_owned()])
            .expect("list rows");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_acknowledged);
    }

    #[test]
    fn aether_trace_cause_logic_resolves_symbol_name_and_file() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"

[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"
"#,
        )
        .expect("write config");

        let server = AetherMcpServer::new(workspace, false).expect("new mcp server");
        let store = SqliteStore::open(workspace).expect("open store");
        let graph = aether_store::CozoGraphStore::open(workspace).expect("open graph");

        let sym_a = SymbolRecord {
            id: "sym-a".to_owned(),
            file_path: "src/a.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "a".to_owned(),
            signature_fingerprint: "sig-a".to_owned(),
            last_seen_at: 1_700_000_000,
        };
        let sym_b = SymbolRecord {
            id: "sym-b".to_owned(),
            file_path: "src/b.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "b".to_owned(),
            signature_fingerprint: "sig-b".to_owned(),
            last_seen_at: 1_700_000_001,
        };
        store.upsert_symbol(sym_a.clone()).expect("upsert a");
        store.upsert_symbol(sym_b.clone()).expect("upsert b");
        graph.upsert_symbol_node(&sym_a).expect("sync a");
        graph.upsert_symbol_node(&sym_b).expect("sync b");
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: sym_a.id.clone(),
                target_id: sym_b.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/a.rs".to_owned(),
            })
            .expect("upsert edge");
        store
            .record_sir_version_if_changed(
                "sym-b",
                "hash-b1",
                "mock",
                "mock",
                r#"{"purpose":"old","edge_cases":[],"dependencies":[]}"#,
                1_700_000_100_000,
                None,
            )
            .expect("insert b history 1");
        store
            .record_sir_version_if_changed(
                "sym-b",
                "hash-b2",
                "mock",
                "mock",
                r#"{"purpose":"new","edge_cases":["edge"],"dependencies":["db"]}"#,
                1_700_000_200_000,
                None,
            )
            .expect("insert b history 2");

        drop(graph);
        let response = server
            .aether_trace_cause_logic(AetherTraceCauseRequest {
                symbol: Some("a".to_owned()),
                symbol_id: None,
                file: Some("src/a.rs".to_owned()),
                lookback: Some("30d".to_owned()),
                max_depth: Some(3),
                limit: Some(3),
            })
            .expect("trace cause");
        assert_eq!(response.schema_version, "1.0");
        assert_eq!(response.target.symbol_id, "sym-a");
    }
}
