use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{DriftConfig, load_workspace_config};
use aether_core::{content_hash, normalize_path};
use aether_infer::{
    EmbeddingProviderOverrides, load_embedding_provider_from_config, summarize_text_with_config,
};
use aether_memory::{EntityRef, NoteSourceType, ProjectMemoryService, RememberRequest};
use aether_store::{
    CommunitySnapshotRecord, CozoGraphStore, DriftAnalysisStateRecord, DriftResultRecord,
    SirHistoryBaselineSelector, SqliteStore, Store, open_vector_store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::coupling::AnalysisError;

const DRIFT_SCHEMA_VERSION: &str = "1.0";
const DRIFT_SUMMARY_SYSTEM_PROMPT: &str = "Summarize how this function's behavior changed given the before/after SIR fields. One sentence, developer audience.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftInclude {
    Semantic,
    Boundary,
    Structural,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DriftReportRequest {
    pub window: Option<String>,
    pub include: Option<Vec<DriftInclude>>,
    pub min_drift_magnitude: Option<f32>,
    pub include_acknowledged: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftReportWindow {
    pub from_commit: String,
    pub to_commit: String,
    pub commit_count: u32,
    pub analyzed_at: i64,
    pub limited_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DriftReportSummary {
    pub symbols_analyzed: u32,
    pub semantic_drifts: u32,
    pub boundary_violations: u32,
    pub emerging_hubs: u32,
    pub new_cycles: u32,
    pub orphaned_subgraphs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DriftTestCoverage {
    pub has_tests: bool,
    pub test_count: u32,
    pub intents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticDriftEntry {
    pub result_id: String,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub drift_magnitude: f32,
    pub similarity: f32,
    pub drift_summary: String,
    pub commit_range: [String; 2],
    pub test_coverage: DriftTestCoverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryViolationEntry {
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmergingHubEntry {
    pub result_id: String,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub current_pagerank: f32,
    pub previous_pagerank: f32,
    pub dependents_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewCycleEntry {
    pub result_id: String,
    pub symbols: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrphanedSubgraphEntry {
    pub result_id: String,
    pub symbols: Vec<String>,
    pub files: Vec<String>,
    pub total_symbols: u32,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StructuralAnomalies {
    pub emerging_hubs: Vec<EmergingHubEntry>,
    pub new_cycles: Vec<NewCycleEntry>,
    pub orphaned_subgraphs: Vec<OrphanedSubgraphEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReportResult {
    pub schema_version: String,
    pub analysis_window: DriftReportWindow,
    pub summary: DriftReportSummary,
    pub semantic_drift: Vec<SemanticDriftEntry>,
    pub boundary_violations: Vec<BoundaryViolationEntry>,
    pub structural_anomalies: StructuralAnomalies,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcknowledgeDriftRequest {
    pub result_ids: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcknowledgeDriftResult {
    pub schema_version: String,
    pub acknowledged: u32,
    pub note_created: bool,
    pub note_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunitiesRequest {
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file_path: String,
    pub community_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunitiesResult {
    pub schema_version: String,
    pub result_count: u32,
    pub communities: Vec<CommunityEntry>,
}

#[derive(Debug, Clone)]
pub struct DriftAnalyzer {
    workspace: PathBuf,
    config: DriftConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowSpec {
    Commits(u32),
    Days(u32),
    SinceCommit(String),
}

#[derive(Debug, Clone)]
struct ResolvedWindow {
    from_commit: String,
    to_commit: String,
    commit_count: u32,
    commits: Vec<String>,
    limited_history: bool,
}

impl DriftAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let config = load_workspace_config(&workspace)?;
        Ok(Self {
            workspace,
            config: config.drift,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &DriftConfig {
        &self.config
    }

    pub fn report(&self, request: DriftReportRequest) -> Result<DriftReportResult, AnalysisError> {
        let analyzed_at = now_millis();
        let include_acknowledged = request.include_acknowledged.unwrap_or(false);
        let min_magnitude = request.min_drift_magnitude.unwrap_or(0.0).clamp(0.0, 1.0);
        let include = effective_includes(request.include.as_ref());
        let window_spec_raw = request
            .window
            .as_deref()
            .unwrap_or(self.config.analysis_window.as_str());
        let window_spec = parse_window_spec(window_spec_raw);
        let resolved_window = self.resolve_window(window_spec)?;

        if !self.config.enabled {
            return Ok(DriftReportResult {
                schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
                analysis_window: DriftReportWindow {
                    from_commit: resolved_window.from_commit,
                    to_commit: resolved_window.to_commit,
                    commit_count: resolved_window.commit_count,
                    analyzed_at,
                    limited_history: resolved_window.limited_history,
                },
                summary: DriftReportSummary::default(),
                semantic_drift: Vec::new(),
                boundary_violations: Vec::new(),
                structural_anomalies: StructuralAnomalies::default(),
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let previous_state = store.get_drift_analysis_state()?;
        let changed_symbols =
            self.collect_changed_symbols(&store, resolved_window.commits.as_slice())?;

        let mut report_records = Vec::<DriftResultRecord>::new();
        let mut snapshot_records = Vec::<DriftResultRecord>::new();

        let semantic_records = self.compute_semantic_records(
            &store,
            changed_symbols.as_slice(),
            &resolved_window,
            min_magnitude,
            analyzed_at,
        )?;
        report_records.extend(semantic_records);

        let boundary =
            self.compute_boundary_records(&store, &cozo, &resolved_window, analyzed_at)?;
        report_records.extend(boundary.records);
        if !boundary.current_snapshot.is_empty() && !resolved_window.to_commit.is_empty() {
            store.replace_community_snapshot(
                resolved_window.to_commit.as_str(),
                analyzed_at,
                boundary.current_snapshot.as_slice(),
            )?;
        }

        let structural = self.compute_structural_records(
            &store,
            &cozo,
            previous_state.as_ref(),
            &resolved_window,
            analyzed_at,
        )?;
        report_records.extend(structural.records);
        snapshot_records.extend(structural.snapshot_records);

        let mut all_records = report_records.clone();
        all_records.extend(snapshot_records);
        store.upsert_drift_results(all_records.as_slice())?;
        store.upsert_drift_analysis_state(DriftAnalysisStateRecord {
            last_analysis_commit: (!resolved_window.to_commit.is_empty())
                .then_some(resolved_window.to_commit.clone()),
            last_analysis_at: Some(analyzed_at),
            symbols_analyzed: changed_symbols.len() as i64,
            drift_detected: report_records.len() as i64,
        })?;

        let report_ids = report_records
            .iter()
            .map(|record| record.result_id.clone())
            .collect::<Vec<_>>();
        let mut persisted = store.list_drift_results_by_ids(report_ids.as_slice())?;
        if !include_acknowledged {
            persisted.retain(|record| !record.is_acknowledged);
        }

        let mut semantic_drift = Vec::new();
        let mut boundary_violations = Vec::new();
        let mut structural_anomalies = StructuralAnomalies::default();

        for record in persisted {
            match record.drift_type.as_str() {
                "semantic" if include.contains(&DriftInclude::Semantic) => {
                    if let Some(entry) = semantic_entry_from_record(record, min_magnitude)? {
                        semantic_drift.push(entry);
                    }
                }
                "boundary_violation" if include.contains(&DriftInclude::Boundary) => {
                    if let Some(entry) = boundary_entry_from_record(record)? {
                        boundary_violations.push(entry);
                    }
                }
                "emerging_hub" | "new_cycle" | "orphaned"
                    if include.contains(&DriftInclude::Structural) =>
                {
                    push_structural_entry(&mut structural_anomalies, record)?;
                }
                _ => {}
            }
        }

        semantic_drift.sort_by(|left, right| {
            right
                .drift_magnitude
                .partial_cmp(&left.drift_magnitude)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        boundary_violations.sort_by(|left, right| {
            left.source_symbol
                .cmp(&right.source_symbol)
                .then_with(|| left.target_symbol.cmp(&right.target_symbol))
                .then_with(|| left.edge_type.cmp(&right.edge_type))
        });
        structural_anomalies.emerging_hubs.sort_by(|left, right| {
            right
                .current_pagerank
                .partial_cmp(&left.current_pagerank)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        structural_anomalies
            .new_cycles
            .sort_by(|left, right| left.symbols.join(",").cmp(&right.symbols.join(",")));
        structural_anomalies
            .orphaned_subgraphs
            .sort_by(|left, right| {
                right
                    .total_symbols
                    .cmp(&left.total_symbols)
                    .then_with(|| left.symbols.join(",").cmp(&right.symbols.join(",")))
            });

        let summary = DriftReportSummary {
            symbols_analyzed: changed_symbols.len() as u32,
            semantic_drifts: semantic_drift.len() as u32,
            boundary_violations: boundary_violations
                .iter()
                .filter(|entry| !entry.informational)
                .count() as u32,
            emerging_hubs: structural_anomalies.emerging_hubs.len() as u32,
            new_cycles: structural_anomalies.new_cycles.len() as u32,
            orphaned_subgraphs: structural_anomalies.orphaned_subgraphs.len() as u32,
        };

        Ok(DriftReportResult {
            schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
            analysis_window: DriftReportWindow {
                from_commit: resolved_window.from_commit,
                to_commit: resolved_window.to_commit,
                commit_count: resolved_window.commit_count,
                analyzed_at,
                limited_history: resolved_window.limited_history,
            },
            summary,
            semantic_drift,
            boundary_violations,
            structural_anomalies,
        })
    }

    pub fn acknowledge_drift(
        &self,
        request: AcknowledgeDriftRequest,
    ) -> Result<AcknowledgeDriftResult, AnalysisError> {
        let result_ids = request
            .result_ids
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if result_ids.is_empty() {
            return Ok(AcknowledgeDriftResult {
                schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
                acknowledged: 0,
                note_created: false,
                note_id: None,
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let records = store.list_drift_results_by_ids(result_ids.as_slice())?;
        let acknowledged = store.acknowledge_drift_results(result_ids.as_slice())?;
        if records.is_empty() || request.note.trim().is_empty() {
            return Ok(AcknowledgeDriftResult {
                schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
                acknowledged,
                note_created: false,
                note_id: None,
            });
        }

        let memory = ProjectMemoryService::new(&self.workspace);
        let mut file_refs = BTreeSet::new();
        let mut symbol_refs = BTreeSet::new();
        let mut entity_refs = Vec::new();
        for record in &records {
            file_refs.insert(record.file_path.clone());
            symbol_refs.insert(record.symbol_id.clone());
            entity_refs.push(EntityRef {
                kind: "drift_result".to_owned(),
                id: record.result_id.clone(),
            });
        }

        let result = memory.remember(RememberRequest {
            content: request.note.trim().to_owned(),
            source_type: NoteSourceType::Agent,
            source_agent: Some("aether_analysis".to_owned()),
            tags: vec!["drift".to_owned(), "acknowledged".to_owned()],
            entity_refs,
            file_refs: file_refs.into_iter().collect(),
            symbol_refs: symbol_refs.into_iter().collect(),
            now_ms: Some(now_millis()),
        })?;

        Ok(AcknowledgeDriftResult {
            schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
            acknowledged,
            note_created: true,
            note_id: Some(result.note.note_id),
        })
    }

    pub fn communities(
        &self,
        _request: CommunitiesRequest,
    ) -> Result<CommunitiesResult, AnalysisError> {
        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let mut entries = cozo
            .list_louvain_communities()?
            .into_iter()
            .map(|(symbol_id, community_id)| {
                let symbol = store.get_symbol_record(symbol_id.as_str())?;
                let (symbol_name, file_path) = symbol
                    .map(|row| (row.qualified_name, row.file_path))
                    .unwrap_or_else(|| (symbol_id.clone(), String::new()));
                Ok(CommunityEntry {
                    symbol_id,
                    symbol_name,
                    file_path,
                    community_id,
                })
            })
            .collect::<Result<Vec<_>, AnalysisError>>()?;

        entries.sort_by(|left, right| {
            left.community_id
                .cmp(&right.community_id)
                .then_with(|| left.file_path.cmp(&right.file_path))
                .then_with(|| left.symbol_name.cmp(&right.symbol_name))
        });

        Ok(CommunitiesResult {
            schema_version: DRIFT_SCHEMA_VERSION.to_owned(),
            result_count: entries.len() as u32,
            communities: entries,
        })
    }

    fn resolve_window(&self, spec: WindowSpec) -> Result<ResolvedWindow, AnalysisError> {
        let repo = match gix::discover(&self.workspace) {
            Ok(repo) => repo,
            Err(_) => {
                return Ok(ResolvedWindow {
                    from_commit: String::new(),
                    to_commit: String::new(),
                    commit_count: 0,
                    commits: Vec::new(),
                    limited_history: false,
                });
            }
        };

        let head_id = repo
            .head_id()
            .map_err(|err| AnalysisError::Git(format!("failed to resolve HEAD: {err}")))?
            .detach();
        let head_commit = head_id.to_string().to_ascii_lowercase();
        let walk = repo
            .rev_walk([head_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(|err| AnalysisError::Git(format!("failed to start revision walk: {err}")))?;

        let cutoff_seconds = match spec {
            WindowSpec::Days(days) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|value| value.as_secs() as i64)
                    .unwrap_or(0);
                Some(now.saturating_sub(days as i64 * 24 * 60 * 60))
            }
            _ => None,
        };
        let mut commits = Vec::new();
        let mut limited_history = false;

        for entry in walk {
            let info = match entry {
                Ok(info) => info,
                Err(err) => {
                    return Err(AnalysisError::Git(format!(
                        "revision walk entry failed: {err}"
                    )));
                }
            };
            let commit_hash = info.id.to_string().to_ascii_lowercase();

            match &spec {
                WindowSpec::Commits(limit) => {
                    if commits.len() >= *limit as usize {
                        limited_history = true;
                        break;
                    }
                    commits.push(commit_hash);
                }
                WindowSpec::Days(_) => {
                    let commit = match repo.find_commit(info.id) {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    let commit_time = commit.time().map(|time| time.seconds).unwrap_or(0);
                    if cutoff_seconds.is_some_and(|cutoff| commit_time < cutoff) {
                        break;
                    }
                    commits.push(commit_hash);
                }
                WindowSpec::SinceCommit(prefix) => {
                    commits.push(commit_hash.clone());
                    if commit_hash.starts_with(prefix.as_str()) {
                        limited_history = true;
                        break;
                    }
                }
            }
        }

        if commits.is_empty() {
            commits.push(head_commit.clone());
        }
        let from_commit = commits
            .last()
            .cloned()
            .unwrap_or_else(|| head_commit.clone());

        Ok(ResolvedWindow {
            from_commit,
            to_commit: head_commit,
            commit_count: commits.len() as u32,
            commits,
            limited_history,
        })
    }

    fn collect_changed_symbols(
        &self,
        store: &SqliteStore,
        commits: &[String],
    ) -> Result<Vec<String>, AnalysisError> {
        let mut symbol_ids = BTreeSet::new();
        for commit_hash in commits {
            let changed_files = self.changed_files_for_commit(commit_hash.as_str())?;
            for file in changed_files {
                for symbol in store.list_symbols_for_file(file.as_str())? {
                    symbol_ids.insert(symbol.id);
                }
            }
        }
        Ok(symbol_ids.into_iter().collect())
    }

    fn changed_files_for_commit(&self, commit_hash: &str) -> Result<Vec<String>, AnalysisError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.workspace)
            .args([
                "diff-tree",
                "--numstat",
                "--no-commit-id",
                "-r",
                "--root",
                commit_hash,
            ])
            .output()?;
        if !output.status.success() {
            return Err(AnalysisError::Git(format!(
                "git diff-tree failed for commit {commit_hash}: status {}",
                output.status
            )));
        }

        let mut files = BTreeSet::new();
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let added = parts.next().unwrap_or_default().trim();
            let removed = parts.next().unwrap_or_default().trim();
            let raw_path = parts.next().unwrap_or_default().trim();
            if raw_path.is_empty() || added == "-" || removed == "-" {
                continue;
            }
            let path = normalize_path(normalize_rename_path(raw_path).as_str());
            if !path.is_empty() {
                files.insert(path);
            }
        }
        Ok(files.into_iter().collect())
    }

    fn compute_semantic_records(
        &self,
        store: &SqliteStore,
        changed_symbols: &[String],
        window: &ResolvedWindow,
        min_magnitude: f32,
        detected_at: i64,
    ) -> Result<Vec<DriftResultRecord>, AnalysisError> {
        if changed_symbols.is_empty() {
            return Ok(Vec::new());
        }

        let Some(loaded) = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?
        else {
            return Ok(Vec::new());
        };
        let provider_name = loaded.provider_name.clone();
        let model_name = loaded.model_name.clone();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| AnalysisError::Git(format!("failed to build tokio runtime: {err}")))?;
        let vector_store = runtime.block_on(open_vector_store(&self.workspace))?;
        let embeddings = runtime.block_on(vector_store.list_embeddings_for_symbols(
            provider_name.as_str(),
            model_name.as_str(),
            changed_symbols,
        ))?;
        let current_embeddings = embeddings
            .into_iter()
            .map(|record| (record.symbol_id, record.embedding))
            .collect::<HashMap<_, _>>();

        let mut records = Vec::new();
        for symbol_id in changed_symbols {
            let Some(symbol) = store.get_symbol_record(symbol_id.as_str())? else {
                continue;
            };
            let Some(current_sir) = store.read_sir_blob(symbol_id.as_str())? else {
                continue;
            };
            let baseline = if window.from_commit.is_empty() {
                store
                    .latest_sir_history_pair(symbol_id.as_str())?
                    .map(|pair| pair.from)
            } else {
                store.resolve_sir_baseline_by_selector(
                    symbol_id.as_str(),
                    SirHistoryBaselineSelector::CommitHash(window.from_commit.clone()),
                )?
            };
            let Some(baseline) = baseline else {
                continue;
            };
            let current_hash = store
                .get_sir_meta(symbol_id.as_str())?
                .map(|meta| meta.sir_hash);

            let current_embedding =
                if let Some(embedding) = current_embeddings.get(symbol_id.as_str()) {
                    embedding.clone()
                } else {
                    let embedded =
                        runtime.block_on(loaded.provider.embed_text(current_sir.as_str()))?;
                    if embedded.is_empty() {
                        continue;
                    }
                    embedded
                };
            let baseline_embedding = if current_hash.as_deref() == Some(baseline.sir_hash.as_str())
            {
                current_embedding.clone()
            } else {
                let embedded =
                    runtime.block_on(loaded.provider.embed_text(baseline.sir_json.as_str()))?;
                if embedded.is_empty() {
                    continue;
                }
                embedded
            };

            let similarity =
                cosine_similarity(current_embedding.as_slice(), baseline_embedding.as_slice());
            if similarity >= self.config.drift_threshold {
                continue;
            }
            let magnitude = (1.0 - similarity).clamp(0.0, 1.0);
            if magnitude < min_magnitude {
                continue;
            }

            let structured_diff =
                build_structured_sir_diff(baseline.sir_json.as_str(), current_sir.as_str())?;
            let mechanical = mechanical_diff_summary(&structured_diff);
            let summary_prompt = json!({
                "before": baseline.sir_json,
                "after": current_sir,
                "structured_diff": structured_diff,
            });
            let llm_summary = runtime
                .block_on(summarize_text_with_config(
                    &self.workspace,
                    DRIFT_SUMMARY_SYSTEM_PROMPT,
                    summary_prompt.to_string().as_str(),
                ))
                .ok()
                .flatten();
            let drift_summary = llm_summary.unwrap_or_else(|| mechanical.clone());

            let test_intents = store
                .list_test_intents_for_symbol(symbol_id.as_str())?
                .into_iter()
                .map(|intent| intent.intent_text)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let test_coverage = json!({
                "has_tests": !test_intents.is_empty(),
                "test_count": test_intents.len() as u32,
                "intents": test_intents,
            });

            let detail_json = serde_json::to_string(&json!({
                "similarity": similarity,
                "structured_diff": structured_diff,
                "mechanical_diff": mechanical,
                "test_coverage": test_coverage,
            }))?;
            let result_id = content_hash(
                format!(
                    "semantic\n{}\n{}\n{}",
                    symbol.id, window.from_commit, window.to_commit
                )
                .as_str(),
            );

            records.push(DriftResultRecord {
                result_id,
                symbol_id: symbol.id,
                file_path: symbol.file_path,
                symbol_name: symbol.qualified_name,
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(magnitude),
                current_sir_hash: current_hash,
                baseline_sir_hash: Some(baseline.sir_hash),
                commit_range_start: Some(window.from_commit.clone()),
                commit_range_end: Some(window.to_commit.clone()),
                drift_summary: Some(drift_summary),
                detail_json,
                detected_at,
                is_acknowledged: false,
            });
        }

        Ok(records)
    }

    fn compute_boundary_records(
        &self,
        store: &SqliteStore,
        cozo: &CozoGraphStore,
        window: &ResolvedWindow,
        detected_at: i64,
    ) -> Result<BoundaryComputation, AnalysisError> {
        let assignments = cozo.list_louvain_communities()?;
        let current_snapshot = assignments
            .iter()
            .map(|(symbol_id, community_id)| CommunitySnapshotRecord {
                snapshot_id: window.to_commit.clone(),
                symbol_id: symbol_id.clone(),
                community_id: *community_id,
                captured_at: detected_at,
            })
            .collect::<Vec<_>>();
        let community_by_symbol = assignments.into_iter().collect::<HashMap<_, _>>();
        let cross_edges = cozo.list_cross_community_edges(&community_by_symbol)?;
        if cross_edges.is_empty() {
            return Ok(BoundaryComputation {
                records: Vec::new(),
                current_snapshot,
            });
        }

        let previous_snapshot = store.list_latest_community_snapshot()?;
        let previous_community_by_symbol = previous_snapshot
            .into_iter()
            .map(|record| (record.symbol_id, record.community_id))
            .collect::<HashMap<_, _>>();
        let first_run = previous_community_by_symbol.is_empty();

        let mut records = Vec::new();
        for (source_id, target_id, edge_type, source_community, target_community) in cross_edges {
            let was_previously_cross = previous_community_by_symbol
                .get(source_id.as_str())
                .zip(previous_community_by_symbol.get(target_id.as_str()))
                .is_some_and(|(left, right)| left != right);
            if !first_run && was_previously_cross {
                continue;
            }

            let source = load_symbol_view(store, source_id.as_str())?;
            let target = load_symbol_view(store, target_id.as_str())?;
            let informational = first_run;
            let note = if informational {
                "Informational baseline: cross-community edge observed on first drift analysis run"
                    .to_owned()
            } else {
                format!(
                    "New cross-community dependency: {} -> {}",
                    source.symbol_name, target.symbol_name
                )
            };
            let detail_json = serde_json::to_string(&json!({
                "source_symbol_id": source.symbol_id,
                "source_symbol": source.symbol_name,
                "source_file": source.file_path,
                "source_community": source_community,
                "target_symbol_id": target.symbol_id,
                "target_symbol": target.symbol_name,
                "target_file": target.file_path,
                "target_community": target_community,
                "edge_type": edge_type,
                "informational": informational,
                "note": note,
            }))?;
            let result_id = content_hash(
                format!(
                    "boundary\n{}\n{}\n{}\n{}",
                    source.symbol_id, target.symbol_id, edge_type, window.to_commit
                )
                .as_str(),
            );
            records.push(DriftResultRecord {
                result_id,
                symbol_id: source.symbol_id,
                file_path: source.file_path,
                symbol_name: source.symbol_name,
                drift_type: "boundary_violation".to_owned(),
                drift_magnitude: None,
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some(window.from_commit.clone()),
                commit_range_end: Some(window.to_commit.clone()),
                drift_summary: Some(note),
                detail_json,
                detected_at,
                is_acknowledged: false,
            });
        }

        Ok(BoundaryComputation {
            records,
            current_snapshot,
        })
    }

    fn compute_structural_records(
        &self,
        store: &SqliteStore,
        cozo: &CozoGraphStore,
        previous_state: Option<&DriftAnalysisStateRecord>,
        window: &ResolvedWindow,
        detected_at: i64,
    ) -> Result<StructuralComputation, AnalysisError> {
        let previous_commit = previous_state
            .and_then(|state| state.last_analysis_commit.clone())
            .unwrap_or_default();
        let existing = if previous_commit.is_empty() {
            Vec::new()
        } else {
            store.list_drift_results(true)?
        };
        let previous_pagerank = existing
            .iter()
            .filter(|record| {
                record.drift_type == "pagerank_snapshot"
                    && record.commit_range_end.as_deref() == Some(previous_commit.as_str())
            })
            .filter_map(|record| {
                let value = serde_json::from_str::<Value>(record.detail_json.as_str()).ok()?;
                let score = value.get("pagerank").and_then(Value::as_f64)?;
                Some((record.symbol_id.clone(), score as f32))
            })
            .collect::<HashMap<_, _>>();
        let previous_cycles = existing
            .iter()
            .filter(|record| {
                record.drift_type == "scc_snapshot"
                    && record.commit_range_end.as_deref() == Some(previous_commit.as_str())
            })
            .filter_map(|record| {
                let value = serde_json::from_str::<Value>(record.detail_json.as_str()).ok()?;
                value
                    .get("cycle_fingerprint")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<HashSet<_>>();

        let mut records = Vec::new();
        let mut snapshot_records = Vec::new();

        let pagerank = cozo.list_pagerank()?;
        if !pagerank.is_empty() {
            let threshold = percentile(
                pagerank
                    .iter()
                    .map(|(_, score)| *score)
                    .collect::<Vec<_>>()
                    .as_slice(),
                self.config.hub_percentile,
            );

            for (symbol_id, score) in &pagerank {
                let snapshot_id = content_hash(
                    format!("pagerank_snapshot\n{}\n{}", symbol_id, window.to_commit).as_str(),
                );
                snapshot_records.push(DriftResultRecord {
                    result_id: snapshot_id,
                    symbol_id: symbol_id.clone(),
                    file_path: String::new(),
                    symbol_name: symbol_id.clone(),
                    drift_type: "pagerank_snapshot".to_owned(),
                    drift_magnitude: Some((*score).clamp(0.0, 1.0)),
                    current_sir_hash: None,
                    baseline_sir_hash: None,
                    commit_range_start: Some(window.from_commit.clone()),
                    commit_range_end: Some(window.to_commit.clone()),
                    drift_summary: None,
                    detail_json: serde_json::to_string(&json!({ "pagerank": score }))?,
                    detected_at,
                    is_acknowledged: true,
                });

                if *score < threshold {
                    continue;
                }
                let previous = previous_pagerank
                    .get(symbol_id.as_str())
                    .copied()
                    .unwrap_or(0.0);
                if previous <= f32::EPSILON {
                    continue;
                }
                let increase_ratio = ((*score - previous) / previous).max(0.0);
                if increase_ratio <= 0.2 {
                    continue;
                }
                let symbol = load_symbol_view(store, symbol_id.as_str())?;
                let dependents_count = store.get_callers(symbol.symbol_name.as_str())?.len() as u32;
                let note = format!(
                    "PageRank increased {:.0}% above previous analysis",
                    increase_ratio * 100.0
                );
                let result_id = content_hash(
                    format!("emerging_hub\n{}\n{}", symbol_id, window.to_commit).as_str(),
                );
                let detail_json = serde_json::to_string(&json!({
                    "current_pagerank": score,
                    "previous_pagerank": previous,
                    "dependents_count": dependents_count,
                    "note": note,
                }))?;
                records.push(DriftResultRecord {
                    result_id,
                    symbol_id: symbol.symbol_id,
                    file_path: symbol.file_path,
                    symbol_name: symbol.symbol_name,
                    drift_type: "emerging_hub".to_owned(),
                    drift_magnitude: Some((score - previous).abs()),
                    current_sir_hash: None,
                    baseline_sir_hash: None,
                    commit_range_start: Some(window.from_commit.clone()),
                    commit_range_end: Some(window.to_commit.clone()),
                    drift_summary: Some(note),
                    detail_json,
                    detected_at,
                    is_acknowledged: false,
                });
            }
        }

        let scc = cozo.list_strongly_connected_components()?;
        for component in scc.into_iter().filter(|component| component.len() > 1) {
            let mut symbols = component.clone();
            symbols.sort();
            let fingerprint = symbols.join(",");
            let snapshot_id = content_hash(
                format!("scc_snapshot\n{}\n{}", fingerprint, window.to_commit).as_str(),
            );
            snapshot_records.push(DriftResultRecord {
                result_id: snapshot_id,
                symbol_id: symbols.first().cloned().unwrap_or_default(),
                file_path: String::new(),
                symbol_name: symbols.first().cloned().unwrap_or_default(),
                drift_type: "scc_snapshot".to_owned(),
                drift_magnitude: None,
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some(window.from_commit.clone()),
                commit_range_end: Some(window.to_commit.clone()),
                drift_summary: None,
                detail_json: serde_json::to_string(&json!({
                    "cycle_fingerprint": fingerprint,
                    "symbols": symbols,
                }))?,
                detected_at,
                is_acknowledged: true,
            });
            if previous_cycles.contains(fingerprint.as_str()) {
                continue;
            }
            let note = "New strongly connected component detected".to_owned();
            let result_id =
                content_hash(format!("new_cycle\n{}\n{}", fingerprint, window.to_commit).as_str());
            records.push(DriftResultRecord {
                result_id,
                symbol_id: symbols.first().cloned().unwrap_or_default(),
                file_path: String::new(),
                symbol_name: symbols.first().cloned().unwrap_or_default(),
                drift_type: "new_cycle".to_owned(),
                drift_magnitude: None,
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some(window.from_commit.clone()),
                commit_range_end: Some(window.to_commit.clone()),
                drift_summary: Some(note.clone()),
                detail_json: serde_json::to_string(&json!({
                    "symbols": symbols,
                    "note": note,
                }))?,
                detected_at,
                is_acknowledged: false,
            });
        }

        let components = cozo.list_connected_components()?;
        if components.len() > 1 {
            let mut iter = components.into_iter();
            let _main_component = iter.next();
            for component in iter {
                if component.is_empty() {
                    continue;
                }
                let files = component
                    .iter()
                    .filter_map(|symbol_id| {
                        store
                            .get_symbol_record(symbol_id.as_str())
                            .ok()
                            .flatten()
                            .map(|symbol| symbol.file_path)
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                let result_id = content_hash(
                    format!("orphaned\n{}\n{}", component.join(","), window.to_commit).as_str(),
                );
                let note =
                    "No dependency edges to main component; orphaned code candidate".to_owned();
                records.push(DriftResultRecord {
                    result_id,
                    symbol_id: component.first().cloned().unwrap_or_default(),
                    file_path: files.first().cloned().unwrap_or_default(),
                    symbol_name: component.first().cloned().unwrap_or_default(),
                    drift_type: "orphaned".to_owned(),
                    drift_magnitude: None,
                    current_sir_hash: None,
                    baseline_sir_hash: None,
                    commit_range_start: Some(window.from_commit.clone()),
                    commit_range_end: Some(window.to_commit.clone()),
                    drift_summary: Some(note.clone()),
                    detail_json: serde_json::to_string(&json!({
                        "symbols": component,
                        "files": files,
                        "note": note,
                    }))?,
                    detected_at,
                    is_acknowledged: false,
                });
            }
        }

        Ok(StructuralComputation {
            records,
            snapshot_records,
        })
    }
}

#[derive(Debug, Clone)]
struct BoundaryComputation {
    records: Vec<DriftResultRecord>,
    current_snapshot: Vec<CommunitySnapshotRecord>,
}

#[derive(Debug, Clone)]
struct StructuralComputation {
    records: Vec<DriftResultRecord>,
    snapshot_records: Vec<DriftResultRecord>,
}

#[derive(Debug, Clone)]
struct SymbolView {
    symbol_id: String,
    symbol_name: String,
    file_path: String,
}

fn load_symbol_view(store: &SqliteStore, symbol_id: &str) -> Result<SymbolView, AnalysisError> {
    let Some(symbol) = store.get_symbol_record(symbol_id)? else {
        return Ok(SymbolView {
            symbol_id: symbol_id.to_owned(),
            symbol_name: symbol_id.to_owned(),
            file_path: String::new(),
        });
    };
    Ok(SymbolView {
        symbol_id: symbol.id,
        symbol_name: symbol.qualified_name,
        file_path: symbol.file_path,
    })
}

fn effective_includes(requested: Option<&Vec<DriftInclude>>) -> HashSet<DriftInclude> {
    let mut includes = requested
        .cloned()
        .unwrap_or_else(|| {
            vec![
                DriftInclude::Semantic,
                DriftInclude::Boundary,
                DriftInclude::Structural,
            ]
        })
        .into_iter()
        .collect::<HashSet<_>>();
    if includes.is_empty() {
        includes.insert(DriftInclude::Semantic);
        includes.insert(DriftInclude::Boundary);
        includes.insert(DriftInclude::Structural);
    }
    includes
}

fn parse_window_spec(value: &str) -> WindowSpec {
    let trimmed = value.trim().to_ascii_lowercase();
    if let Some(commit) = trimmed.strip_prefix("since:") {
        let commit = commit.trim().to_owned();
        if !commit.is_empty() {
            return WindowSpec::SinceCommit(commit);
        }
    }
    if let Some(days) = trimmed.strip_suffix('d')
        && let Ok(days) = days.trim().parse::<u32>()
    {
        return WindowSpec::Days(days.max(1));
    }
    let first_token = trimmed.split_whitespace().next().unwrap_or_default();
    let commits = first_token.parse::<u32>().unwrap_or(100).max(1);
    WindowSpec::Commits(commits)
}

fn normalize_rename_path(path: &str) -> String {
    let value = path.trim();
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim()
            .to_owned();
    }
    value.to_owned()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut left_norm_sq = 0.0f32;
    let mut right_norm_sq = 0.0f32;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        dot += left_value * right_value;
        left_norm_sq += left_value * left_value;
        right_norm_sq += right_value * right_value;
    }
    if left_norm_sq <= f32::EPSILON || right_norm_sq <= f32::EPSILON {
        return 0.0;
    }
    (dot / (left_norm_sq.sqrt() * right_norm_sq.sqrt())).clamp(0.0, 1.0)
}

fn field_string(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(field) = value.get(*key)
            && let Some(text) = field.as_str()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_owned();
            }
        }
    }
    String::new()
}

fn field_strings(value: &Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(field) = value.get(*key) else {
            continue;
        };
        let Some(items) = field.as_array() else {
            continue;
        };
        let values = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

fn build_structured_sir_diff(before_json: &str, after_json: &str) -> Result<Value, AnalysisError> {
    let before = serde_json::from_str::<Value>(before_json)?;
    let after = serde_json::from_str::<Value>(after_json)?;
    let before_purpose = field_string(&before, &["purpose", "intent"]);
    let after_purpose = field_string(&after, &["purpose", "intent"]);
    let before_edge_cases = field_strings(&before, &["edge_cases", "error_modes"]);
    let after_edge_cases = field_strings(&after, &["edge_cases", "error_modes"]);
    let before_constraints = field_strings(&before, &["constraints"]);
    let after_constraints = field_strings(&after, &["constraints"]);

    let edge_cases_added = after_edge_cases
        .iter()
        .filter(|item| !before_edge_cases.contains(item))
        .cloned()
        .collect::<Vec<_>>();
    let edge_cases_removed = before_edge_cases
        .iter()
        .filter(|item| !after_edge_cases.contains(item))
        .cloned()
        .collect::<Vec<_>>();
    let constraints_added = after_constraints
        .iter()
        .filter(|item| !before_constraints.contains(item))
        .cloned()
        .collect::<Vec<_>>();
    let constraints_removed = before_constraints
        .iter()
        .filter(|item| !after_constraints.contains(item))
        .cloned()
        .collect::<Vec<_>>();

    Ok(json!({
        "purpose": {
            "before": before_purpose,
            "after": after_purpose,
            "changed": before_purpose != after_purpose,
        },
        "edge_cases": {
            "before": before_edge_cases,
            "after": after_edge_cases,
            "added": edge_cases_added,
            "removed": edge_cases_removed,
            "changed": before_edge_cases != after_edge_cases,
        },
        "constraints": {
            "before": before_constraints,
            "after": after_constraints,
            "added": constraints_added,
            "removed": constraints_removed,
            "changed": before_constraints != after_constraints,
        }
    }))
}

fn mechanical_diff_summary(structured_diff: &Value) -> String {
    let mut parts = Vec::new();
    if structured_diff
        .pointer("/purpose/changed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let before = structured_diff
            .pointer("/purpose/before")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let after = structured_diff
            .pointer("/purpose/after")
            .and_then(Value::as_str)
            .unwrap_or_default();
        parts.push(format!("purpose changed: '{before}' -> '{after}'"));
    }
    let edge_added = structured_diff
        .pointer("/edge_cases/added")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !edge_added.is_empty() {
        parts.push(format!("edge_cases added: {}", edge_added.join(", ")));
    }
    let edge_removed = structured_diff
        .pointer("/edge_cases/removed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !edge_removed.is_empty() {
        parts.push(format!("edge_cases removed: {}", edge_removed.join(", ")));
    }
    let constraints_added = structured_diff
        .pointer("/constraints/added")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !constraints_added.is_empty() {
        parts.push(format!(
            "constraints added: {}",
            constraints_added.join(", ")
        ));
    }
    let constraints_removed = structured_diff
        .pointer("/constraints/removed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !constraints_removed.is_empty() {
        parts.push(format!(
            "constraints removed: {}",
            constraints_removed.join(", ")
        ));
    }

    if parts.is_empty() {
        "semantic fields changed".to_owned()
    } else {
        parts.join("; ")
    }
}

fn semantic_entry_from_record(
    record: DriftResultRecord,
    min_magnitude: f32,
) -> Result<Option<SemanticDriftEntry>, AnalysisError> {
    let Some(magnitude) = record.drift_magnitude else {
        return Ok(None);
    };
    if magnitude < min_magnitude {
        return Ok(None);
    }
    let detail = serde_json::from_str::<Value>(record.detail_json.as_str())?;
    let similarity = detail
        .get("similarity")
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or_else(|| (1.0 - magnitude).clamp(0.0, 1.0));
    let test_coverage = detail
        .get("test_coverage")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let test_intents = test_coverage
        .get("intents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Some(SemanticDriftEntry {
        result_id: record.result_id,
        symbol_id: record.symbol_id,
        symbol_name: record.symbol_name,
        file: record.file_path,
        drift_magnitude: magnitude,
        similarity,
        drift_summary: record
            .drift_summary
            .unwrap_or_else(|| "semantic drift detected".to_owned()),
        commit_range: [
            record.commit_range_start.unwrap_or_default(),
            record.commit_range_end.unwrap_or_default(),
        ],
        test_coverage: DriftTestCoverage {
            has_tests: test_coverage
                .get("has_tests")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            test_count: test_coverage
                .get("test_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            intents: test_intents,
        },
    }))
}

fn boundary_entry_from_record(
    record: DriftResultRecord,
) -> Result<Option<BoundaryViolationEntry>, AnalysisError> {
    let detail = serde_json::from_str::<Value>(record.detail_json.as_str())?;
    let Some(source_symbol) = detail.get("source_symbol").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(target_symbol) = detail.get("target_symbol").and_then(Value::as_str) else {
        return Ok(None);
    };
    Ok(Some(BoundaryViolationEntry {
        result_id: record.result_id,
        source_symbol: source_symbol.to_owned(),
        source_file: detail
            .get("source_file")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        source_community: detail
            .get("source_community")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        target_symbol: target_symbol.to_owned(),
        target_file: detail
            .get("target_file")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        target_community: detail
            .get("target_community")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        edge_type: detail
            .get("edge_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        first_seen_commit: record.commit_range_end.unwrap_or_default(),
        informational: detail
            .get("informational")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        note: detail
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    }))
}

fn push_structural_entry(
    anomalies: &mut StructuralAnomalies,
    record: DriftResultRecord,
) -> Result<(), AnalysisError> {
    let detail = serde_json::from_str::<Value>(record.detail_json.as_str())?;
    match record.drift_type.as_str() {
        "emerging_hub" => {
            anomalies.emerging_hubs.push(EmergingHubEntry {
                result_id: record.result_id,
                symbol_id: record.symbol_id,
                symbol_name: record.symbol_name,
                file: record.file_path,
                current_pagerank: detail
                    .get("current_pagerank")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
                previous_pagerank: detail
                    .get("previous_pagerank")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
                dependents_count: detail
                    .get("dependents_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                note: detail
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        "new_cycle" => {
            anomalies.new_cycles.push(NewCycleEntry {
                result_id: record.result_id,
                symbols: detail
                    .get("symbols")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                note: detail
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        "orphaned" => {
            let symbols = detail
                .get("symbols")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            anomalies.orphaned_subgraphs.push(OrphanedSubgraphEntry {
                result_id: record.result_id,
                files: detail
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                total_symbols: symbols.len() as u32,
                symbols,
                note: detail
                    .get("note")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        _ => {}
    }
    Ok(())
}

fn percentile(values: &[f32], percentile: u32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let pct = percentile.clamp(1, 100) as f32 / 100.0;
    let idx = ((sorted.len() as f32 - 1.0) * pct).round() as usize;
    sorted[idx.min(sorted.len().saturating_sub(1))]
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use aether_store::{
        CozoGraphStore, DriftResultRecord, GraphStore, SirMetaRecord, SqliteStore, Store,
        SymbolRecord,
    };
    use tempfile::tempdir;

    use super::{
        AcknowledgeDriftRequest, DriftAnalyzer, DriftInclude, DriftReportRequest, now_millis,
        parse_window_spec,
    };

    #[test]
    fn parse_window_spec_supports_commits_days_and_since_commit() {
        assert!(matches!(
            parse_window_spec("50 commits"),
            super::WindowSpec::Commits(50)
        ));
        assert!(matches!(
            parse_window_spec("14d"),
            super::WindowSpec::Days(14)
        ));
        assert!(matches!(
            parse_window_spec("since:abc123"),
            super::WindowSpec::SinceCommit(_)
        ));
    }

    #[test]
    fn report_generates_semantic_drift_and_mechanical_fallback() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());
        write_file(temp.path(), "src/lib.rs", "fn run() {}\n");
        commit_all(temp.path(), "initial");
        let first_commit = git_output(temp.path(), &["rev-parse", "HEAD"]);

        write_file(
            temp.path(),
            "src/lib.rs",
            "fn run() { println!(\"hi\"); }\n",
        );
        commit_all(temp.path(), "updated");
        let head_commit = git_output(temp.path(), &["rev-parse", "HEAD"]);

        let analyzer = DriftAnalyzer::new(temp.path()).expect("create analyzer");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-run".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::run".to_owned(),
                signature_fingerprint: "sig-run".to_owned(),
                last_seen_at: now_millis(),
            })
            .expect("upsert symbol");
        store
            .record_sir_version_if_changed(
                "sym-run",
                "hash-v1",
                "mock",
                "mock",
                r#"{"purpose":"initial behavior","edge_cases":["none"],"constraints":[]}"#,
                now_millis().saturating_sub(1_000),
                Some(first_commit.as_str()),
            )
            .expect("insert baseline");
        store
            .record_sir_version_if_changed(
                "sym-run",
                "hash-v2",
                "mock",
                "mock",
                r#"{"purpose":"batch processing behavior","edge_cases":["large batch"],"constraints":["must be transactional"]}"#,
                now_millis(),
                Some(head_commit.as_str()),
            )
            .expect("insert latest history");
        store
            .write_sir_blob(
                "sym-run",
                r#"{"purpose":"batch processing behavior","edge_cases":["large batch"],"constraints":["must be transactional"]}"#,
            )
            .expect("write current sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: "sym-run".to_owned(),
                sir_hash: "hash-v2".to_owned(),
                sir_version: 2,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                updated_at: now_millis(),
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: now_millis(),
            })
            .expect("upsert sir meta");

        let report = analyzer
            .report(DriftReportRequest {
                window: Some("2 commits".to_owned()),
                include: Some(vec![DriftInclude::Semantic]),
                min_drift_magnitude: Some(0.0),
                include_acknowledged: Some(false),
            })
            .expect("run drift report");
        assert_eq!(report.semantic_drift.len(), 1);
        assert!(report.semantic_drift[0].drift_magnitude > 0.0);
        assert!(!report.semantic_drift[0].drift_summary.is_empty());
    }

    #[test]
    fn first_run_boundary_edges_are_informational() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());
        write_file(temp.path(), "src/lib.rs", "fn run() {}\n");
        commit_all(temp.path(), "initial");

        let analyzer = DriftAnalyzer::new(temp.path()).expect("create analyzer");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let cozo = CozoGraphStore::open(temp.path()).expect("open cozo");

        let source = SymbolRecord {
            id: "sym-a".to_owned(),
            file_path: "src/a.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "a".to_owned(),
            signature_fingerprint: "sig-a".to_owned(),
            last_seen_at: now_millis(),
        };
        let target = SymbolRecord {
            id: "sym-b".to_owned(),
            file_path: "src/b.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "b".to_owned(),
            signature_fingerprint: "sig-b".to_owned(),
            last_seen_at: now_millis(),
        };
        store.upsert_symbol(source.clone()).expect("upsert source");
        store.upsert_symbol(target.clone()).expect("upsert target");
        cozo.upsert_symbol_node(&source).expect("sync source");
        cozo.upsert_symbol_node(&target).expect("sync target");
        cozo.upsert_edge(&aether_store::ResolvedEdge {
            source_id: source.id.clone(),
            target_id: target.id.clone(),
            edge_kind: aether_core::EdgeKind::Calls,
            file_path: "src/a.rs".to_owned(),
        })
        .expect("upsert edge");
        drop(cozo);

        let report = analyzer
            .report(DriftReportRequest {
                window: Some("1 commits".to_owned()),
                include: Some(vec![DriftInclude::Boundary]),
                min_drift_magnitude: None,
                include_acknowledged: Some(false),
            })
            .expect("run boundary report");
        if !report.boundary_violations.is_empty() {
            assert!(report.boundary_violations[0].informational);
        }
    }

    #[test]
    fn acknowledge_flow_marks_results_and_creates_note() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());
        let analyzer = DriftAnalyzer::new(temp.path()).expect("create analyzer");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-ack-1".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "src/a.rs".to_owned(),
                symbol_name: "a".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.3),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some(String::new()),
                commit_range_end: Some(String::new()),
                drift_summary: Some("changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: now_millis(),
                is_acknowledged: false,
            }])
            .expect("seed drift result");

        let result = analyzer
            .acknowledge_drift(AcknowledgeDriftRequest {
                result_ids: vec!["drift-ack-1".to_owned()],
                note: "Intentional drift".to_owned(),
            })
            .expect("acknowledge drift");
        assert_eq!(result.acknowledged, 1);
        assert!(result.note_created);
        assert!(result.note_id.is_some());
    }

    #[test]
    fn report_handles_missing_history_and_embeddings_gracefully() {
        let temp = tempdir().expect("tempdir");
        init_repo_without_embeddings(temp.path());
        write_file(temp.path(), "src/lib.rs", "fn run() {}\n");
        commit_all(temp.path(), "initial");

        let analyzer = DriftAnalyzer::new(temp.path()).expect("create analyzer");
        let report = analyzer
            .report(DriftReportRequest {
                window: Some("1 commits".to_owned()),
                include: Some(vec![DriftInclude::Semantic, DriftInclude::Structural]),
                min_drift_magnitude: Some(0.0),
                include_acknowledged: Some(false),
            })
            .expect("run report");
        assert!(report.semantic_drift.is_empty());
    }

    fn init_repo(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["config", "user.email", "tester@example.com"]);
        run_git(workspace, &["config", "user.name", "Tester"]);
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = true
provider = "mock"
vector_backend = "sqlite"

[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[drift]
enabled = true
drift_threshold = 0.99
analysis_window = "50 commits"
auto_analyze = false
hub_percentile = 95
"#,
        )
        .expect("write config");
    }

    fn init_repo_without_embeddings(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["config", "user.email", "tester@example.com"]);
        run_git(workspace, &["config", "user.name", "Tester"]);
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = false
provider = "mock"
vector_backend = "sqlite"

[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[drift]
enabled = true
drift_threshold = 0.85
analysis_window = "50 commits"
auto_analyze = false
hub_percentile = 95
"#,
        )
        .expect("write config");
    }

    fn write_file(workspace: &Path, relative: &str, content: &str) {
        let path = workspace.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, content).expect("write file");
    }

    fn commit_all(workspace: &Path, message: &str) {
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", message]);
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn git_output(workspace: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}
