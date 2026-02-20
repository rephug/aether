use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{CouplingConfig, GraphBackend, load_workspace_config};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{
    CouplingEdgeRecord, CouplingMiningStateRecord, CozoGraphStore, SqliteStore, Store, StoreError,
    open_vector_store,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const STALE_COMMIT_THRESHOLD: i64 = 100;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("config error: {0}")]
    Config(#[from] aether_config::ConfigError),
    #[error("store error: {0}")]
    Store(#[from] StoreError),
    #[error("inference error: {0}")]
    Infer(#[from] aether_infer::InferError),
    #[error("memory error: {0}")]
    Memory(#[from] aether_memory::MemoryError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git error: {0}")]
    Git(String),
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn min_score(self) -> f32 {
        match self {
            Self::Low => 0.0,
            Self::Medium => 0.2,
            Self::High => 0.4,
            Self::Critical => 0.7,
        }
    }

    pub fn from_score(score: f32) -> Self {
        if score >= Self::Critical.min_score() {
            return Self::Critical;
        }
        if score >= Self::High.min_score() {
            return Self::High;
        }
        if score >= Self::Medium.min_score() {
            return Self::Medium;
        }
        Self::Low
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouplingType {
    Structural,
    Temporal,
    Semantic,
    HiddenOperational,
    Multi,
}

impl CouplingType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structural => "structural",
            Self::Temporal => "temporal",
            Self::Semantic => "semantic",
            Self::HiddenOperational => "hidden_operational",
            Self::Multi => "multi",
        }
    }

    fn parse(value: &str) -> Self {
        match value.trim() {
            "structural" => Self::Structural,
            "semantic" => Self::Semantic,
            "hidden_operational" => Self::HiddenOperational,
            "multi" => Self::Multi,
            _ => Self::Temporal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalBreakdown {
    pub temporal: f32,
    pub static_signal: f32,
    pub semantic: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingEdge {
    pub file_a: String,
    pub file_b: String,
    pub co_change_count: i64,
    pub total_commits_a: i64,
    pub total_commits_b: i64,
    pub fused_score: f32,
    pub coupling_type: CouplingType,
    pub signals: SignalBreakdown,
    pub last_co_change_commit: String,
    pub last_co_change_at: i64,
    pub mined_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingMiningOutcome {
    pub mined: bool,
    pub git_repo_found: bool,
    pub head_commit_hash: Option<String>,
    pub commits_scanned: i64,
    pub pairs_upserted: usize,
    pub mined_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MineCouplingRequest {
    pub commits: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlastRadiusRequest {
    pub file_path: String,
    pub min_risk: RiskLevel,
    pub auto_mine: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlastRadiusEntry {
    pub file: String,
    pub risk_level: RiskLevel,
    pub fused_score: f32,
    pub coupling_type: CouplingType,
    pub signals: SignalBreakdown,
    pub co_change_count: i64,
    pub total_commits: i64,
    pub last_co_change_commit: String,
    pub last_co_change_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlastRadiusResult {
    pub target_file: String,
    pub mining_state: Option<CouplingMiningStateRecord>,
    pub coupled_files: Vec<BlastRadiusEntry>,
}

#[derive(Debug, Clone)]
pub struct CouplingAnalyzer {
    workspace: PathBuf,
    config: CouplingConfig,
    graph_backend: GraphBackend,
}

#[derive(Debug, Clone)]
struct PairAggregate {
    count: i64,
    last_commit_hash: String,
    last_commit_at: i64,
}

#[derive(Debug, Clone)]
struct EmbeddingContext {
    by_file: HashMap<String, Vec<Vec<f32>>>,
}

impl CouplingAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let config = load_workspace_config(&workspace)?;
        Ok(Self {
            workspace,
            config: config.coupling,
            graph_backend: config.storage.graph_backend,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn config(&self) -> &CouplingConfig {
        &self.config
    }

    pub fn mine(
        &self,
        request: MineCouplingRequest,
    ) -> Result<CouplingMiningOutcome, AnalysisError> {
        let mined_at = now_millis();
        if !self.config.enabled {
            return Ok(CouplingMiningOutcome {
                mined: false,
                git_repo_found: true,
                head_commit_hash: None,
                commits_scanned: 0,
                pairs_upserted: 0,
                mined_at,
            });
        }

        let store = SqliteStore::open(&self.workspace)?;
        let repo = match gix::discover(&self.workspace) {
            Ok(repo) => repo,
            Err(_) => {
                return Ok(CouplingMiningOutcome {
                    mined: false,
                    git_repo_found: false,
                    head_commit_hash: None,
                    commits_scanned: 0,
                    pairs_upserted: 0,
                    mined_at,
                });
            }
        };

        let head_id = repo
            .head_id()
            .map_err(|err| AnalysisError::Git(format!("failed to resolve HEAD: {err}")))?
            .detach();
        let head_commit_hash = head_id.to_string().to_ascii_lowercase();
        let previous_state = store.get_coupling_mining_state()?;
        let stop_hash = previous_state
            .as_ref()
            .and_then(|state| state.last_commit_hash.clone());
        let commit_window = request.commits.unwrap_or(self.config.commit_window).max(1);

        let mut per_file_commit_count = HashMap::<String, i64>::new();
        let mut pairs = BTreeMap::<(String, String), PairAggregate>::new();
        let mut commits_scanned = 0i64;

        let walk = repo
            .rev_walk([head_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(|err| AnalysisError::Git(format!("failed to start revision walk: {err}")))?;

        for entry in walk {
            if commits_scanned >= i64::from(commit_window) {
                break;
            }

            let info = match entry {
                Ok(info) => info,
                Err(err) => {
                    return Err(AnalysisError::Git(format!(
                        "revision walk entry failed: {err}"
                    )));
                }
            };
            let commit_hash = info.id.to_string().to_ascii_lowercase();

            if stop_hash
                .as_deref()
                .is_some_and(|last_hash| last_hash == commit_hash)
            {
                break;
            }
            commits_scanned += 1;

            let commit = match repo.find_commit(info.id) {
                Ok(commit) => commit,
                Err(_) => continue,
            };
            let parent_count = commit.parent_ids().count();
            if parent_count != 1 {
                continue;
            }
            let commit_at = commit.time().map(|time| time.seconds).unwrap_or(0);

            let changed_files = self.changed_files_for_commit(commit_hash.as_str())?;
            if changed_files.is_empty() {
                continue;
            }
            if changed_files.len() as u32 > self.config.bulk_commit_threshold {
                continue;
            }

            for file in &changed_files {
                *per_file_commit_count.entry(file.clone()).or_insert(0) += 1;
            }

            let sorted_files = changed_files
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for left_idx in 0..sorted_files.len() {
                for right_idx in (left_idx + 1)..sorted_files.len() {
                    let file_a = sorted_files[left_idx].clone();
                    let file_b = sorted_files[right_idx].clone();
                    pairs
                        .entry((file_a, file_b))
                        .and_modify(|agg| {
                            agg.count += 1;
                        })
                        .or_insert(PairAggregate {
                            count: 1,
                            last_commit_hash: commit_hash.clone(),
                            last_commit_at: commit_at,
                        });
                }
            }
        }

        let cozo = CozoGraphStore::open(&self.workspace)?;
        let mut candidate_pairs = Vec::new();
        for ((file_a, file_b), aggregate) in pairs {
            let total_commits_a = per_file_commit_count
                .get(file_a.as_str())
                .copied()
                .unwrap_or(0);
            let total_commits_b = per_file_commit_count
                .get(file_b.as_str())
                .copied()
                .unwrap_or(0);

            let mut co_change_count = aggregate.count;
            let mut merged_total_commits_a = total_commits_a;
            let mut merged_total_commits_b = total_commits_b;

            if let Some(existing) = cozo.get_co_change_edge(file_a.as_str(), file_b.as_str())? {
                co_change_count += existing.co_change_count;
                merged_total_commits_a += existing.total_commits_a;
                merged_total_commits_b += existing.total_commits_b;
            }

            if co_change_count < i64::from(self.config.min_co_change_count) {
                continue;
            }

            let denominator = merged_total_commits_a.max(merged_total_commits_b).max(1) as f32;
            let temporal_signal = (co_change_count as f32 / denominator).clamp(0.0, 1.0);
            candidate_pairs.push((
                file_a,
                file_b,
                co_change_count,
                merged_total_commits_a,
                merged_total_commits_b,
                temporal_signal,
                aggregate.last_commit_hash,
                aggregate.last_commit_at,
            ));
        }

        let embedding_context = self.build_embedding_context(
            &store,
            candidate_pairs
                .iter()
                .flat_map(|pair| [pair.0.as_str(), pair.1.as_str()]),
        )?;

        let mut records = Vec::new();
        for (
            file_a,
            file_b,
            co_change_count,
            total_commits_a,
            total_commits_b,
            temporal_signal,
            last_co_change_commit,
            last_co_change_at,
        ) in candidate_pairs
        {
            let static_signal = if self.has_static_dependency(&store, &cozo, &file_a, &file_b)? {
                1.0
            } else {
                0.0
            };
            let semantic_signal = embedding_context.max_similarity(&file_a, &file_b);
            let fused_score = fused_score(temporal_signal, static_signal, semantic_signal);
            let coupling_type =
                classify_coupling_type(temporal_signal, static_signal, semantic_signal).as_str();

            records.push(CouplingEdgeRecord {
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling: temporal_signal,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type: coupling_type.to_owned(),
                last_co_change_commit,
                last_co_change_at,
                mined_at,
            });
        }

        cozo.upsert_co_change_edges(records.as_slice())?;
        store.upsert_coupling_mining_state(CouplingMiningStateRecord {
            last_commit_hash: Some(head_commit_hash.clone()),
            last_mined_at: Some(mined_at),
            commits_scanned: previous_state
                .map(|state| state.commits_scanned.max(0))
                .unwrap_or(0)
                .saturating_add(commits_scanned),
        })?;

        Ok(CouplingMiningOutcome {
            mined: true,
            git_repo_found: true,
            head_commit_hash: Some(head_commit_hash),
            commits_scanned,
            pairs_upserted: records.len(),
            mined_at,
        })
    }

    pub fn blast_radius(
        &self,
        request: BlastRadiusRequest,
    ) -> Result<BlastRadiusResult, AnalysisError> {
        let file_path = normalize_repo_path(request.file_path.as_str());
        let mut store = SqliteStore::open(&self.workspace)?;
        let mut mining_state = store.get_coupling_mining_state()?;

        if request.auto_mine && self.needs_auto_mine(mining_state.as_ref())? {
            let _ = self.mine(MineCouplingRequest::default())?;
            store = SqliteStore::open(&self.workspace)?;
            mining_state = store.get_coupling_mining_state()?;
        }

        let cozo = CozoGraphStore::open(&self.workspace)?;
        let edges =
            cozo.list_co_change_edges_for_file(file_path.as_str(), request.min_risk.min_score())?;

        let mut coupled_files = Vec::with_capacity(edges.len());
        for edge in edges {
            let (file, total_commits) = if edge.file_a == file_path {
                (edge.file_b.clone(), edge.total_commits_b)
            } else {
                (edge.file_a.clone(), edge.total_commits_a)
            };

            coupled_files.push(BlastRadiusEntry {
                file,
                risk_level: RiskLevel::from_score(edge.fused_score),
                fused_score: edge.fused_score,
                coupling_type: CouplingType::parse(edge.coupling_type.as_str()),
                signals: SignalBreakdown {
                    temporal: edge.git_coupling,
                    static_signal: edge.static_signal,
                    semantic: edge.semantic_signal,
                },
                co_change_count: edge.co_change_count,
                total_commits,
                last_co_change_commit: edge.last_co_change_commit,
                last_co_change_at: edge.last_co_change_at,
            });
        }

        coupled_files.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file.cmp(&right.file))
        });

        Ok(BlastRadiusResult {
            target_file: file_path,
            mining_state,
            coupled_files,
        })
    }

    pub fn coupling_report(&self, top: u32) -> Result<Vec<CouplingEdge>, AnalysisError> {
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let records = cozo.list_top_co_change_edges(top)?;
        Ok(records
            .into_iter()
            .map(|record| CouplingEdge {
                file_a: record.file_a,
                file_b: record.file_b,
                co_change_count: record.co_change_count,
                total_commits_a: record.total_commits_a,
                total_commits_b: record.total_commits_b,
                fused_score: record.fused_score,
                coupling_type: CouplingType::parse(record.coupling_type.as_str()),
                signals: SignalBreakdown {
                    temporal: record.git_coupling,
                    static_signal: record.static_signal,
                    semantic: record.semantic_signal,
                },
                last_co_change_commit: record.last_co_change_commit,
                last_co_change_at: record.last_co_change_at,
                mined_at: record.mined_at,
            })
            .collect())
    }

    pub fn commits_since_last_mine(
        &self,
        state: Option<&CouplingMiningStateRecord>,
    ) -> Result<i64, AnalysisError> {
        let Some(last_commit_hash) = state
            .and_then(|value| value.last_commit_hash.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(i64::MAX);
        };

        let repo = match gix::discover(&self.workspace) {
            Ok(repo) => repo,
            Err(_) => return Ok(0),
        };
        let head_id = repo
            .head_id()
            .map_err(|err| AnalysisError::Git(format!("failed to resolve HEAD: {err}")))?
            .detach();

        let walk = repo
            .rev_walk([head_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(|err| AnalysisError::Git(format!("failed to start revision walk: {err}")))?;

        let mut count = 0i64;
        for entry in walk {
            let info = match entry {
                Ok(info) => info,
                Err(_) => continue,
            };
            let commit_hash = info.id.to_string().to_ascii_lowercase();
            if commit_hash == last_commit_hash {
                break;
            }
            count += 1;
        }

        Ok(count)
    }

    fn needs_auto_mine(
        &self,
        state: Option<&CouplingMiningStateRecord>,
    ) -> Result<bool, AnalysisError> {
        if state.is_none() {
            return Ok(true);
        }

        Ok(self.commits_since_last_mine(state)? > STALE_COMMIT_THRESHOLD)
    }

    fn has_static_dependency(
        &self,
        store: &SqliteStore,
        cozo: &CozoGraphStore,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, AnalysisError> {
        if self.graph_backend == GraphBackend::Cozo
            && cozo.has_dependency_between_files(file_a, file_b)?
        {
            return Ok(true);
        }

        store
            .has_dependency_between_files(file_a, file_b)
            .map_err(Into::into)
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

            let path = normalize_repo_path(normalize_rename_path(raw_path).as_str());
            if path.is_empty() || self.is_excluded(path.as_str()) {
                continue;
            }

            files.insert(path);
        }

        Ok(files.into_iter().collect())
    }

    fn is_excluded(&self, path: &str) -> bool {
        if path.is_empty() {
            return true;
        }

        let file_name = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(path);

        self.config.exclude_patterns.iter().any(|pattern| {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                return false;
            }

            wildcard_match(pattern, path)
                || (!pattern.contains('/') && wildcard_match(pattern, file_name))
        })
    }

    fn build_embedding_context<'a>(
        &self,
        store: &SqliteStore,
        files: impl Iterator<Item = &'a str>,
    ) -> Result<EmbeddingContext, AnalysisError> {
        let mut by_file = HashMap::<String, Vec<Vec<f32>>>::new();

        let Some(loaded) = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?
        else {
            return Ok(EmbeddingContext { by_file });
        };
        let provider = loaded.provider_name;
        let model = loaded.model_name;
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Ok(EmbeddingContext { by_file });
        }

        let unique_files = files
            .map(normalize_repo_path)
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if unique_files.is_empty() {
            return Ok(EmbeddingContext { by_file });
        }

        let mut symbol_ids_by_file = HashMap::<String, Vec<String>>::new();
        let mut all_symbol_ids = Vec::new();
        for file in &unique_files {
            let symbols = store.list_symbols_for_file(file.as_str())?;
            let symbol_ids = symbols
                .into_iter()
                .map(|symbol| symbol.id)
                .collect::<Vec<_>>();
            all_symbol_ids.extend(symbol_ids.iter().cloned());
            symbol_ids_by_file.insert(file.clone(), symbol_ids);
        }
        if all_symbol_ids.is_empty() {
            return Ok(EmbeddingContext { by_file });
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| AnalysisError::Git(format!("failed to build tokio runtime: {err}")))?;
        let vector_store = runtime.block_on(open_vector_store(&self.workspace))?;
        let records = runtime.block_on(vector_store.list_embeddings_for_symbols(
            provider.as_str(),
            model.as_str(),
            all_symbol_ids.as_slice(),
        ))?;
        let embedding_by_symbol = records
            .into_iter()
            .map(|record| (record.symbol_id, record.embedding))
            .collect::<HashMap<_, _>>();

        for (file, symbol_ids) in symbol_ids_by_file {
            let vectors = symbol_ids
                .into_iter()
                .filter_map(|symbol_id| embedding_by_symbol.get(symbol_id.as_str()).cloned())
                .collect::<Vec<_>>();
            by_file.insert(file, vectors);
        }

        Ok(EmbeddingContext { by_file })
    }
}

impl EmbeddingContext {
    fn max_similarity(&self, file_a: &str, file_b: &str) -> f32 {
        let Some(left) = self.by_file.get(file_a) else {
            return 0.0;
        };
        let Some(right) = self.by_file.get(file_b) else {
            return 0.0;
        };

        let mut max_score = 0.0f32;
        for left_embedding in left {
            for right_embedding in right {
                let score =
                    cosine_similarity(left_embedding.as_slice(), right_embedding.as_slice());
                if score > max_score {
                    max_score = score;
                }
            }
        }
        max_score.clamp(0.0, 1.0)
    }
}

pub fn fused_score(temporal_signal: f32, static_signal: f32, semantic_signal: f32) -> f32 {
    (0.5 * temporal_signal) + (0.3 * static_signal) + (0.2 * semantic_signal)
}

pub fn classify_coupling_type(
    temporal_signal: f32,
    static_signal: f32,
    semantic_signal: f32,
) -> CouplingType {
    if static_signal > 0.0 && temporal_signal >= 0.2 {
        return CouplingType::Multi;
    }
    if static_signal > 0.0 && temporal_signal < 0.2 {
        return CouplingType::Structural;
    }
    if static_signal == 0.0 && semantic_signal >= 0.3 {
        return CouplingType::Semantic;
    }
    if static_signal == 0.0 && semantic_signal < 0.3 && temporal_signal >= 0.5 {
        return CouplingType::HiddenOperational;
    }
    CouplingType::Temporal
}

fn normalize_repo_path(path: &str) -> String {
    aether_core::normalize_path(path.trim())
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

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut previous = vec![false; text.len() + 1];
    let mut current = vec![false; text.len() + 1];
    previous[0] = true;

    for &token in pattern {
        current[0] = token == b'*' && previous[0];
        for index in 1..=text.len() {
            current[index] = match token {
                b'*' => current[index - 1] || previous[index],
                b'?' => previous[index - 1],
                _ => previous[index - 1] && token == text[index - 1],
            };
        }
        std::mem::swap(&mut previous, &mut current);
        current.fill(false);
    }

    previous[text.len()]
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

    use super::{
        CouplingAnalyzer, CouplingType, MineCouplingRequest, classify_coupling_type, fused_score,
    };
    use tempfile::tempdir;

    #[test]
    fn fused_score_uses_expected_weights() {
        let fused = fused_score(0.8, 1.0, 0.5);
        assert!((fused - 0.8).abs() < 1e-6);
    }

    #[test]
    fn coupling_type_classification_covers_all_categories() {
        assert_eq!(classify_coupling_type(0.4, 1.0, 0.1), CouplingType::Multi);
        assert_eq!(
            classify_coupling_type(0.1, 1.0, 0.2),
            CouplingType::Structural
        );
        assert_eq!(
            classify_coupling_type(0.1, 0.0, 0.4),
            CouplingType::Semantic
        );
        assert_eq!(
            classify_coupling_type(0.6, 0.0, 0.2),
            CouplingType::HiddenOperational
        );
        assert_eq!(
            classify_coupling_type(0.3, 0.0, 0.1),
            CouplingType::Temporal
        );
    }

    #[test]
    fn mining_skips_bulk_commits_and_supports_incremental_resume() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());

        write_file(temp.path(), "src/a.rs", "fn a() {}\n");
        write_file(temp.path(), "src/b.rs", "fn b() {}\n");
        commit_all(temp.path(), "initial");

        write_file(temp.path(), "src/a.rs", "fn a() { println!(\"a\"); }\n");
        write_file(temp.path(), "src/b.rs", "fn b() { println!(\"b\"); }\n");
        commit_all(temp.path(), "cochange-1");

        write_file(temp.path(), "src/a.rs", "fn a() { println!(\"a2\"); }\n");
        write_file(temp.path(), "src/b.rs", "fn b() { println!(\"b2\"); }\n");
        commit_all(temp.path(), "cochange-2");

        for idx in 0..31 {
            write_file(
                temp.path(),
                &format!("src/generated_{idx}.rs"),
                &format!("fn generated_{idx}() {{}}\n"),
            );
        }
        write_file(
            temp.path(),
            "src/a.rs",
            "fn a() { println!(\"bulk-a\"); }\n",
        );
        write_file(
            temp.path(),
            "src/b.rs",
            "fn b() { println!(\"bulk-b\"); }\n",
        );
        commit_all(temp.path(), "bulk-commit");

        let analyzer = CouplingAnalyzer::new(temp.path()).expect("analyzer");
        let first = analyzer
            .mine(MineCouplingRequest { commits: Some(100) })
            .expect("first mine");
        assert!(first.mined);
        assert!(first.pairs_upserted >= 1);
        let report = analyzer.coupling_report(5).expect("coupling report");
        let pair = report
            .into_iter()
            .find(|edge| edge.file_a == "src/a.rs" && edge.file_b == "src/b.rs")
            .expect("a-b edge should exist");
        assert_eq!(pair.co_change_count, 2);

        write_file(temp.path(), "src/a.rs", "fn a() { println!(\"a3\"); }\n");
        write_file(temp.path(), "src/b.rs", "fn b() { println!(\"b3\"); }\n");
        commit_all(temp.path(), "cochange-3");

        let second = analyzer
            .mine(MineCouplingRequest { commits: Some(100) })
            .expect("second mine");
        assert!(second.mined);
        assert!(second.commits_scanned <= 2);
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

[coupling]
enabled = true
commit_window = 500
min_co_change_count = 2
exclude_patterns = ["*.lock", "*.generated.*", ".gitignore"]
bulk_commit_threshold = 30
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
}
