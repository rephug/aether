use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::load_workspace_config;
use aether_infer::{
    EmbeddingProviderOverrides, LoadedEmbeddingProvider, load_embedding_provider_from_config,
};
use aether_store::{CozoGraphStore, SirHistoryRecord, SqliteStore, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coupling::AnalysisError;
use crate::drift::{
    build_structured_sir_diff, cosine_similarity, structural_change_magnitude_from_diff,
};

const CAUSAL_SCHEMA_VERSION: &str = "1.0";
const DEFAULT_LOOKBACK: &str = "20 commits";
const DEFAULT_MAX_DEPTH: u32 = 5;
const MAX_MAX_DEPTH: u32 = 10;
const DEFAULT_LIMIT: u32 = 5;
const MAX_LIMIT: u32 = 50;
const MILLIS_PER_DAY: f32 = 86_400_000.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TraceCauseRequest {
    pub target_symbol_id: String,
    pub lookback: Option<String>,
    pub max_depth: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCauseTarget {
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCauseAnalysisWindow {
    pub lookback: String,
    pub max_depth: u32,
    pub upstream_symbols_scanned: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalChainSirDiff {
    pub purpose_changed: bool,
    pub purpose_before: String,
    pub purpose_after: String,
    pub edge_cases_added: Vec<String>,
    pub edge_cases_removed: Vec<String>,
    pub dependencies_added: Vec<String>,
    pub dependencies_removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalChainChange {
    pub commit: String,
    pub author: String,
    pub date: String,
    pub change_magnitude: f32,
    pub sir_diff: CausalChainSirDiff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalChainCoupling {
    pub fused_score: f32,
    pub coupling_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausalChainEntry {
    pub rank: u32,
    pub causal_score: f32,
    pub symbol_id: String,
    pub symbol_name: String,
    pub file: String,
    pub dependency_path: Vec<String>,
    pub depth: u32,
    pub change: CausalChainChange,
    pub coupling: CausalChainCoupling,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceCauseResult {
    pub schema_version: String,
    pub target: TraceCauseTarget,
    pub analysis_window: TraceCauseAnalysisWindow,
    pub causal_chain: Vec<CausalChainEntry>,
    pub no_change_upstream: u32,
    pub skipped_missing_history: u32,
    pub embedding_fallback_count: u32,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CausalAnalyzer {
    workspace: PathBuf,
}

#[derive(Debug, Clone)]
enum LookbackSpec {
    Commits(u32),
    Days(u32),
    SinceCommit(String),
}

#[derive(Debug, Clone)]
struct CommitMetadata {
    author: String,
    date: String,
    timestamp_ms: i64,
}

#[derive(Debug, Clone)]
enum LookbackContext {
    Commits {
        commit_set: HashSet<String>,
        commit_metadata: HashMap<String, CommitMetadata>,
        lower_bound_ms: Option<i64>,
    },
    Days {
        cutoff_ms: i64,
    },
}

impl LookbackContext {
    fn contains_history_entry(&self, row: &SirHistoryRecord) -> bool {
        match self {
            Self::Commits {
                commit_set,
                lower_bound_ms,
                ..
            } => {
                if let Some(commit_hash) = row.commit_hash.as_deref()
                    && commit_hash_matches(commit_hash, commit_set)
                {
                    return true;
                }
                lower_bound_ms.is_some_and(|bound| row.created_at >= bound)
            }
            Self::Days { cutoff_ms } => row.created_at >= *cutoff_ms,
        }
    }

    fn commit_metadata_for_hash(&self, commit_hash: &str) -> Option<&CommitMetadata> {
        match self {
            Self::Commits {
                commit_metadata, ..
            } => lookup_commit_metadata(commit_metadata, commit_hash),
            Self::Days { .. } => None,
        }
    }
}

impl CausalAnalyzer {
    pub fn new(workspace: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let workspace = workspace.as_ref().to_path_buf();
        let _ = load_workspace_config(&workspace)?;
        Ok(Self { workspace })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn trace_cause(
        &self,
        request: TraceCauseRequest,
    ) -> Result<TraceCauseResult, AnalysisError> {
        let target_symbol_id = request.target_symbol_id.trim();
        if target_symbol_id.is_empty() {
            return Err(AnalysisError::Message(
                "target_symbol_id is required for trace-cause".to_owned(),
            ));
        }

        let lookback = request
            .lookback
            .as_deref()
            .unwrap_or(DEFAULT_LOOKBACK)
            .trim()
            .to_owned();
        let requested_max_depth = request.max_depth.unwrap_or(DEFAULT_MAX_DEPTH);
        let max_depth = requested_max_depth.clamp(1, MAX_MAX_DEPTH);
        let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

        let store = SqliteStore::open(&self.workspace)?;
        let cozo = CozoGraphStore::open(&self.workspace)?;
        let Some(target_symbol) = store.get_symbol_record(target_symbol_id)? else {
            return Err(AnalysisError::Message(format!(
                "target symbol '{target_symbol_id}' not found"
            )));
        };

        let traversal = cozo.list_upstream_dependency_traversal(target_symbol_id, max_depth)?;
        let mut notes = Vec::new();
        if requested_max_depth > MAX_MAX_DEPTH {
            notes.push(format!("max depth truncated at {max_depth}"));
        }
        if traversal.nodes.is_empty() {
            notes.push("no upstream dependencies".to_owned());
            return Ok(TraceCauseResult {
                schema_version: CAUSAL_SCHEMA_VERSION.to_owned(),
                target: TraceCauseTarget {
                    symbol_id: target_symbol.id,
                    symbol_name: symbol_leaf_name(target_symbol.qualified_name.as_str()),
                    file: target_symbol.file_path,
                },
                analysis_window: TraceCauseAnalysisWindow {
                    lookback,
                    max_depth,
                    upstream_symbols_scanned: 0,
                },
                causal_chain: Vec::new(),
                no_change_upstream: 0,
                skipped_missing_history: 0,
                embedding_fallback_count: 0,
                notes,
            });
        }

        let lookback_context = self.resolve_lookback_context(lookback.as_str())?;
        let mut adjacency = HashMap::<String, Vec<String>>::new();
        for edge in &traversal.edges {
            adjacency
                .entry(edge.source_id.clone())
                .or_default()
                .push(edge.target_id.clone());
        }
        for neighbors in adjacency.values_mut() {
            neighbors.sort();
            neighbors.dedup();
        }

        let (depth_by_symbol, parent_by_symbol) =
            build_shortest_path_tree(target_symbol_id, &adjacency, max_depth);
        let mut symbol_records = HashMap::new();
        symbol_records.insert(target_symbol.id.clone(), target_symbol.clone());
        for symbol_id in depth_by_symbol.keys() {
            if let Some(symbol) = store.get_symbol_record(symbol_id.as_str())? {
                symbol_records.insert(symbol_id.clone(), symbol);
            }
        }

        let embedding_provider = load_embedding_provider_from_config(
            &self.workspace,
            EmbeddingProviderOverrides::default(),
        )?;
        let runtime = if embedding_provider.is_some() {
            Some(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|err| {
                        AnalysisError::Git(format!("failed to build tokio runtime: {err}"))
                    })?,
            )
        } else {
            None
        };

        let now_ms = now_millis();
        let mut no_change_upstream = 0u32;
        let mut skipped_missing_history = 0u32;
        let mut embedding_fallback_count = 0u32;
        let mut candidates = Vec::new();

        let mut ordered_symbols = depth_by_symbol
            .iter()
            .map(|(symbol_id, depth)| (symbol_id.clone(), *depth))
            .collect::<Vec<_>>();
        ordered_symbols
            .sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));

        for (symbol_id, depth) in ordered_symbols {
            if symbol_id == target_symbol.id {
                continue;
            }
            if depth == 0 || depth > max_depth {
                continue;
            }

            let history = store.list_sir_history(symbol_id.as_str())?;
            if history.len() < 2 {
                skipped_missing_history = skipped_missing_history.saturating_add(1);
                continue;
            }
            let Some(pair_index) = latest_change_index_in_window(&history, &lookback_context)
            else {
                no_change_upstream = no_change_upstream.saturating_add(1);
                continue;
            };
            if pair_index == 0 {
                no_change_upstream = no_change_upstream.saturating_add(1);
                continue;
            }

            let before = history[pair_index - 1].clone();
            let after = history[pair_index].clone();
            let structured_diff =
                build_structured_sir_diff(before.sir_json.as_str(), after.sir_json.as_str())?;
            let sir_diff = causal_sir_diff_from_structured(&structured_diff);

            let (change_magnitude, used_embedding_fallback) = compute_change_magnitude(
                &runtime,
                embedding_provider.as_ref(),
                before.sir_json.as_str(),
                after.sir_json.as_str(),
                &structured_diff,
            );
            if used_embedding_fallback {
                embedding_fallback_count = embedding_fallback_count.saturating_add(1);
            }

            let change_metadata = resolve_change_metadata(&after, &lookback_context);
            let days_since_change = (now_ms.saturating_sub(change_metadata.timestamp_ms).max(0)
                as f32)
                / MILLIS_PER_DAY;
            let recency_weight = 1.0 / (1.0 + days_since_change);

            let Some(candidate_symbol) = symbol_records.get(symbol_id.as_str()).cloned() else {
                skipped_missing_history = skipped_missing_history.saturating_add(1);
                continue;
            };
            let (coupling_strength, coupling_type) = resolve_coupling_strength(
                &cozo,
                target_symbol.file_path.as_str(),
                candidate_symbol.file_path.as_str(),
                depth,
            )?;
            let causal_score =
                (recency_weight * coupling_strength * change_magnitude).clamp(0.0, 1.0);

            let path_ids = build_path_ids(
                target_symbol.id.as_str(),
                symbol_id.as_str(),
                &parent_by_symbol,
            )
            .unwrap_or_else(|| vec![target_symbol.id.clone(), symbol_id.clone()]);
            let dependency_path = path_ids
                .into_iter()
                .map(|id| {
                    symbol_records
                        .get(id.as_str())
                        .map(|record| symbol_leaf_name(record.qualified_name.as_str()))
                        .unwrap_or(id)
                })
                .collect::<Vec<_>>();

            candidates.push(CausalChainEntry {
                rank: 0,
                causal_score,
                symbol_id: candidate_symbol.id,
                symbol_name: symbol_leaf_name(candidate_symbol.qualified_name.as_str()),
                file: candidate_symbol.file_path,
                dependency_path,
                depth,
                change: CausalChainChange {
                    commit: change_metadata.commit,
                    author: change_metadata.author,
                    date: change_metadata.date,
                    change_magnitude,
                    sir_diff,
                },
                coupling: CausalChainCoupling {
                    fused_score: coupling_strength,
                    coupling_type,
                },
            });
        }

        if candidates.is_empty() && no_change_upstream > 0 {
            notes.push("no semantic changes in window".to_owned());
        }
        if skipped_missing_history > 0 {
            notes.push(format!(
                "skipped {skipped_missing_history} upstream symbols with insufficient sir history"
            ));
        }

        candidates.sort_by(|left, right| {
            right
                .causal_score
                .partial_cmp(&left.causal_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.depth.cmp(&right.depth))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        candidates.truncate(limit as usize);
        for (idx, row) in candidates.iter_mut().enumerate() {
            row.rank = idx as u32 + 1;
        }

        Ok(TraceCauseResult {
            schema_version: CAUSAL_SCHEMA_VERSION.to_owned(),
            target: TraceCauseTarget {
                symbol_id: target_symbol.id,
                symbol_name: symbol_leaf_name(target_symbol.qualified_name.as_str()),
                file: target_symbol.file_path,
            },
            analysis_window: TraceCauseAnalysisWindow {
                lookback,
                max_depth,
                upstream_symbols_scanned: traversal.nodes.len() as u32,
            },
            causal_chain: candidates,
            no_change_upstream,
            skipped_missing_history,
            embedding_fallback_count,
            notes,
        })
    }

    fn resolve_lookback_context(&self, raw: &str) -> Result<LookbackContext, AnalysisError> {
        match parse_lookback_spec(raw) {
            LookbackSpec::Days(days) => {
                let now_ms = now_millis();
                let cutoff_ms =
                    now_ms.saturating_sub(i64::from(days).saturating_mul(24 * 60 * 60 * 1000));
                Ok(LookbackContext::Days { cutoff_ms })
            }
            LookbackSpec::Commits(limit) => self.resolve_commit_lookback(limit as usize, None),
            LookbackSpec::SinceCommit(prefix) => {
                self.resolve_commit_lookback(usize::MAX, Some(prefix))
            }
        }
    }

    fn resolve_commit_lookback(
        &self,
        limit: usize,
        stop_prefix: Option<String>,
    ) -> Result<LookbackContext, AnalysisError> {
        let repo = match gix::discover(&self.workspace) {
            Ok(repo) => repo,
            Err(_) => {
                return Ok(LookbackContext::Commits {
                    commit_set: HashSet::new(),
                    commit_metadata: HashMap::new(),
                    lower_bound_ms: None,
                });
            }
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

        let mut hashes = HashSet::new();
        let mut metadata = HashMap::new();
        let mut lower_bound_ms = None;
        let stop_prefix = stop_prefix.map(|value| value.trim().to_ascii_lowercase());

        for entry in walk {
            if hashes.len() >= limit {
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
            let commit = match repo.find_commit(info.id) {
                Ok(commit) => commit,
                Err(_) => continue,
            };
            let author = commit
                .author()
                .ok()
                .map(|sig| String::from_utf8_lossy(sig.name.as_ref()).trim().to_owned())
                .unwrap_or_default();
            let (date, timestamp_ms) = commit
                .time()
                .map(|value| {
                    (
                        value.format_or_unix(gix::date::time::format::ISO8601_STRICT),
                        value.seconds.saturating_mul(1000),
                    )
                })
                .unwrap_or_else(|_| (String::new(), 0));

            hashes.insert(commit_hash.clone());
            metadata.insert(
                commit_hash.clone(),
                CommitMetadata {
                    author,
                    date,
                    timestamp_ms,
                },
            );
            lower_bound_ms = Some(timestamp_ms);

            if stop_prefix
                .as_deref()
                .is_some_and(|prefix| commit_hash.starts_with(prefix))
            {
                break;
            }
        }

        Ok(LookbackContext::Commits {
            commit_set: hashes,
            commit_metadata: metadata,
            lower_bound_ms,
        })
    }
}

fn parse_lookback_spec(value: &str) -> LookbackSpec {
    let trimmed = value.trim().to_ascii_lowercase();
    if let Some(commit) = trimmed.strip_prefix("since:") {
        let commit = commit.trim().to_owned();
        if !commit.is_empty() {
            return LookbackSpec::SinceCommit(commit);
        }
    }
    if let Some(days) = trimmed.strip_suffix('d')
        && let Ok(days) = days.trim().parse::<u32>()
    {
        return LookbackSpec::Days(days.max(1));
    }
    let first_token = trimmed.split_whitespace().next().unwrap_or_default();
    let commits = first_token.parse::<u32>().unwrap_or(20).max(1);
    LookbackSpec::Commits(commits)
}

fn latest_change_index_in_window(
    history: &[SirHistoryRecord],
    context: &LookbackContext,
) -> Option<usize> {
    (1..history.len())
        .rev()
        .find(|idx| context.contains_history_entry(&history[*idx]))
}

fn commit_hash_matches(candidate: &str, commit_set: &HashSet<String>) -> bool {
    let normalized = candidate.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    if commit_set.contains(normalized.as_str()) {
        return true;
    }
    commit_set
        .iter()
        .any(|hash| hash.starts_with(normalized.as_str()) || normalized.starts_with(hash))
}

fn lookup_commit_metadata<'a>(
    commit_metadata: &'a HashMap<String, CommitMetadata>,
    commit_hash: &str,
) -> Option<&'a CommitMetadata> {
    let normalized = commit_hash.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if let Some(record) = commit_metadata.get(normalized.as_str()) {
        return Some(record);
    }
    commit_metadata.iter().find_map(|(hash, value)| {
        (hash.starts_with(normalized.as_str()) || normalized.starts_with(hash.as_str()))
            .then_some(value)
    })
}

fn build_shortest_path_tree(
    start: &str,
    adjacency: &HashMap<String, Vec<String>>,
    max_depth: u32,
) -> (HashMap<String, u32>, HashMap<String, String>) {
    let mut depth_by_symbol = HashMap::<String, u32>::new();
    let mut parent_by_symbol = HashMap::<String, String>::new();
    let mut queue = VecDeque::<(String, u32)>::new();

    depth_by_symbol.insert(start.to_owned(), 0);
    queue.push_back((start.to_owned(), 0));
    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for neighbor in adjacency.get(current.as_str()).into_iter().flatten() {
            if depth_by_symbol.contains_key(neighbor.as_str()) {
                continue;
            }
            let next_depth = depth + 1;
            depth_by_symbol.insert(neighbor.clone(), next_depth);
            parent_by_symbol.insert(neighbor.clone(), current.clone());
            queue.push_back((neighbor.clone(), next_depth));
        }
    }

    (depth_by_symbol, parent_by_symbol)
}

fn build_path_ids(
    start_symbol_id: &str,
    target_symbol_id: &str,
    parent_by_symbol: &HashMap<String, String>,
) -> Option<Vec<String>> {
    let mut path = vec![target_symbol_id.to_owned()];
    let mut current = target_symbol_id.to_owned();
    while current != start_symbol_id {
        let parent = parent_by_symbol.get(current.as_str())?.clone();
        path.push(parent.clone());
        current = parent;
    }
    path.reverse();
    Some(path)
}

fn resolve_coupling_strength(
    cozo: &CozoGraphStore,
    target_file: &str,
    upstream_file: &str,
    depth: u32,
) -> Result<(f32, String), AnalysisError> {
    if !target_file.is_empty() && !upstream_file.is_empty() {
        let mut edge = cozo.get_co_change_edge(target_file, upstream_file)?;
        if edge.is_none() {
            edge = cozo.get_co_change_edge(upstream_file, target_file)?;
        }
        if let Some(edge) = edge {
            return Ok((edge.fused_score.clamp(0.0, 1.0), edge.coupling_type));
        }
    }

    let fallback = 0.5 * (1.0 / depth.max(1) as f32);
    Ok((fallback.clamp(0.0, 1.0), "depth_fallback".to_owned()))
}

fn compute_change_magnitude(
    runtime: &Option<tokio::runtime::Runtime>,
    embedding_provider: Option<&LoadedEmbeddingProvider>,
    before_sir: &str,
    after_sir: &str,
    structured_diff: &Value,
) -> (f32, bool) {
    let Some(runtime) = runtime else {
        return (structural_change_magnitude_from_diff(structured_diff), true);
    };
    let Some(embedding_provider) = embedding_provider else {
        return (structural_change_magnitude_from_diff(structured_diff), true);
    };

    let before_embedding = runtime.block_on(embedding_provider.provider.embed_text(before_sir));
    let after_embedding = runtime.block_on(embedding_provider.provider.embed_text(after_sir));
    match (before_embedding, after_embedding) {
        (Ok(before), Ok(after))
            if !before.is_empty() && !after.is_empty() && before.len() == after.len() =>
        {
            let similarity = cosine_similarity(before.as_slice(), after.as_slice());
            ((1.0 - similarity).clamp(0.0, 1.0), false)
        }
        _ => (structural_change_magnitude_from_diff(structured_diff), true),
    }
}

#[derive(Debug, Clone)]
struct ChangeMetadata {
    commit: String,
    author: String,
    date: String,
    timestamp_ms: i64,
}

fn resolve_change_metadata(after: &SirHistoryRecord, context: &LookbackContext) -> ChangeMetadata {
    let commit = after.commit_hash.clone().unwrap_or_default();
    if let Some(metadata) = context.commit_metadata_for_hash(commit.as_str()) {
        return ChangeMetadata {
            commit,
            author: metadata.author.clone(),
            date: metadata.date.clone(),
            timestamp_ms: metadata.timestamp_ms.max(0),
        };
    }

    ChangeMetadata {
        commit,
        author: String::new(),
        date: after.created_at.to_string(),
        timestamp_ms: after.created_at.max(0),
    }
}

fn causal_sir_diff_from_structured(structured_diff: &Value) -> CausalChainSirDiff {
    CausalChainSirDiff {
        purpose_changed: structured_diff
            .pointer("/purpose/changed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        purpose_before: structured_diff
            .pointer("/purpose/before")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        purpose_after: structured_diff
            .pointer("/purpose/after")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        edge_cases_added: pointer_to_string_vec(structured_diff, "/edge_cases/added"),
        edge_cases_removed: pointer_to_string_vec(structured_diff, "/edge_cases/removed"),
        dependencies_added: pointer_to_string_vec(structured_diff, "/dependencies/added"),
        dependencies_removed: pointer_to_string_vec(structured_diff, "/dependencies/removed"),
    }
}

fn pointer_to_string_vec(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
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
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use aether_core::EdgeKind;
    use aether_store::{CozoGraphStore, GraphStore, ResolvedEdge, Store, SymbolRecord};
    use tempfile::tempdir;

    use super::{CausalAnalyzer, TraceCauseRequest, now_millis};

    #[test]
    fn trace_cause_returns_expected_causal_chain_and_dependency_path() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let commit_a = commit_marker(temp.path(), "a");
        let commit_b = commit_marker(temp.path(), "b");
        let commit_c = commit_marker(temp.path(), "c");

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");

        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        let sym_c = symbol("sym-c", "c", "src/c.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_symbol_and_node(&store, &graph, &sym_c);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_b.id, &sym_c.id, EdgeKind::DependsOn);

        record_history(
            &store,
            &sym_b.id,
            "hash-b1",
            r#"{"purpose":"old b","edge_cases":[],"dependencies":[]}"#,
            Some(commit_a.as_str()),
            now_millis().saturating_sub(3_000_000),
        );
        record_history(
            &store,
            &sym_b.id,
            "hash-b2",
            r#"{"purpose":"new b","edge_cases":["b edge"],"dependencies":["db"]}"#,
            Some(commit_b.as_str()),
            now_millis().saturating_sub(2_000_000),
        );
        record_history(
            &store,
            &sym_c.id,
            "hash-c1",
            r#"{"purpose":"old c","edge_cases":[],"dependencies":[]}"#,
            Some(commit_a.as_str()),
            now_millis().saturating_sub(3_000_000),
        );
        record_history(
            &store,
            &sym_c.id,
            "hash-c2",
            r#"{"purpose":"new c","edge_cases":["c edge"],"dependencies":["cache"]}"#,
            Some(commit_c.as_str()),
            now_millis().saturating_sub(1_000_000),
        );

        graph
            .upsert_co_change_edges(&[aether_store::CouplingEdgeRecord {
                file_a: "src/a.rs".to_owned(),
                file_b: "src/b.rs".to_owned(),
                co_change_count: 4,
                total_commits_a: 5,
                total_commits_b: 4,
                git_coupling: 0.7,
                static_signal: 0.8,
                semantic_signal: 0.6,
                fused_score: 0.8,
                coupling_type: "multi".to_owned(),
                last_co_change_commit: commit_b.clone(),
                last_co_change_at: 1_700_000_000,
                mined_at: 1_700_000_100,
            }])
            .expect("upsert co-change");

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("5 commits".to_owned()),
                max_depth: Some(5),
                limit: Some(5),
            })
            .expect("trace cause");

        assert_eq!(result.analysis_window.upstream_symbols_scanned, 2);
        assert!(result.causal_chain.iter().any(|entry| {
            entry.symbol_id == sym_b.id
                && entry.dependency_path == vec!["a".to_owned(), "b".to_owned()]
        }));
        assert!(result.causal_chain.iter().any(|entry| {
            entry.symbol_id == sym_c.id
                && entry.dependency_path == vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        }));
    }

    #[test]
    fn trace_cause_ranking_prefers_recency_coupling_and_magnitude() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let _ = commit_marker(temp.path(), "seed");
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");

        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        let sym_c = symbol("sym-c", "c", "src/c.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_symbol_and_node(&store, &graph, &sym_c);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_a.id, &sym_c.id, EdgeKind::Calls);

        let now = now_millis();
        record_history(
            &store,
            &sym_b.id,
            "hash-b1",
            r#"{"purpose":"old b","edge_cases":[],"dependencies":[]}"#,
            None,
            now.saturating_sub(3 * 86_400_000),
        );
        record_history(
            &store,
            &sym_b.id,
            "hash-b2",
            r#"{"purpose":"new b","edge_cases":["one","two"],"dependencies":["db","cache"]}"#,
            None,
            now.saturating_sub(1 * 86_400_000),
        );
        record_history(
            &store,
            &sym_c.id,
            "hash-c1",
            r#"{"purpose":"old c","edge_cases":[],"dependencies":[]}"#,
            None,
            now.saturating_sub(20 * 86_400_000),
        );
        record_history(
            &store,
            &sym_c.id,
            "hash-c2",
            r#"{"purpose":"old c","edge_cases":["minor"],"dependencies":[]}"#,
            None,
            now.saturating_sub(15 * 86_400_000),
        );

        graph
            .upsert_co_change_edges(&[
                aether_store::CouplingEdgeRecord {
                    file_a: "src/a.rs".to_owned(),
                    file_b: "src/b.rs".to_owned(),
                    co_change_count: 8,
                    total_commits_a: 10,
                    total_commits_b: 10,
                    git_coupling: 0.9,
                    static_signal: 0.9,
                    semantic_signal: 0.9,
                    fused_score: 0.9,
                    coupling_type: "multi".to_owned(),
                    last_co_change_commit: String::new(),
                    last_co_change_at: 1_700_000_000,
                    mined_at: 1_700_000_100,
                },
                aether_store::CouplingEdgeRecord {
                    file_a: "src/a.rs".to_owned(),
                    file_b: "src/c.rs".to_owned(),
                    co_change_count: 2,
                    total_commits_a: 10,
                    total_commits_b: 5,
                    git_coupling: 0.2,
                    static_signal: 0.2,
                    semantic_signal: 0.2,
                    fused_score: 0.2,
                    coupling_type: "multi".to_owned(),
                    last_co_change_commit: String::new(),
                    last_co_change_at: 1_700_000_000,
                    mined_at: 1_700_000_100,
                },
            ])
            .expect("upsert co-change");

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("90d".to_owned()),
                max_depth: Some(3),
                limit: Some(5),
            })
            .expect("trace cause");
        assert!(!result.causal_chain.is_empty());
        assert_eq!(result.causal_chain[0].symbol_id, sym_b.id);
    }

    #[test]
    fn trace_cause_uses_fallback_coupling_for_uncoupled_symbols() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let _ = commit_marker(temp.path(), "seed");
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");

        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        let sym_c = symbol("sym-c", "c", "src/c.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_symbol_and_node(&store, &graph, &sym_c);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_b.id, &sym_c.id, EdgeKind::Calls);

        let now = now_millis();
        record_history(
            &store,
            &sym_c.id,
            "hash-c1",
            r#"{"purpose":"old c","edge_cases":[],"dependencies":[]}"#,
            None,
            now.saturating_sub(10_000),
        );
        record_history(
            &store,
            &sym_c.id,
            "hash-c2",
            r#"{"purpose":"new c","edge_cases":["timeout"],"dependencies":["queue"]}"#,
            None,
            now.saturating_sub(5_000),
        );

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("30d".to_owned()),
                max_depth: Some(3),
                limit: Some(5),
            })
            .expect("trace cause");
        let entry = result
            .causal_chain
            .iter()
            .find(|entry| entry.symbol_id == sym_c.id)
            .expect("entry for c");
        assert!((entry.coupling.fused_score - 0.25).abs() < 0.001);
        assert_eq!(entry.coupling.coupling_type, "depth_fallback");
    }

    #[test]
    fn trace_cause_handles_cycles_with_depth_limit() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let _ = commit_marker(temp.path(), "seed");
        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");

        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        let sym_c = symbol("sym-c", "c", "src/c.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_symbol_and_node(&store, &graph, &sym_c);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_b.id, &sym_c.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_c.id, &sym_b.id, EdgeKind::DependsOn);

        let now = now_millis();
        record_history(
            &store,
            &sym_b.id,
            "hash-b1",
            r#"{"purpose":"old b","edge_cases":[],"dependencies":[]}"#,
            None,
            now.saturating_sub(4_000),
        );
        record_history(
            &store,
            &sym_b.id,
            "hash-b2",
            r#"{"purpose":"new b","edge_cases":["edge"],"dependencies":["db"]}"#,
            None,
            now.saturating_sub(2_000),
        );

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("7d".to_owned()),
                max_depth: Some(3),
                limit: Some(10),
            })
            .expect("trace cause");
        assert!(result.causal_chain.len() <= 2);
        assert!(result.causal_chain.iter().all(|row| row.depth <= 3));
    }

    #[test]
    fn trace_cause_returns_empty_when_no_changes_are_in_window() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let commit_old = commit_marker(temp.path(), "old");
        let _commit_new = commit_marker(temp.path(), "new");

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");
        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);

        record_history(
            &store,
            &sym_b.id,
            "hash-b1",
            r#"{"purpose":"old b","edge_cases":[],"dependencies":[]}"#,
            Some(commit_old.as_str()),
            now_millis().saturating_sub(100_000),
        );
        record_history(
            &store,
            &sym_b.id,
            "hash-b2",
            r#"{"purpose":"new b","edge_cases":["edge"],"dependencies":["db"]}"#,
            Some(commit_old.as_str()),
            now_millis().saturating_sub(90_000),
        );

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("1 commits".to_owned()),
                max_depth: Some(3),
                limit: Some(5),
            })
            .expect("trace cause");
        assert!(result.causal_chain.is_empty());
        assert!(
            result
                .notes
                .iter()
                .any(|note| note.contains("no semantic changes in window"))
        );
    }

    #[test]
    fn trace_cause_skips_missing_history_and_uses_embedding_fallback() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path(), false);
        let _ = commit_marker(temp.path(), "seed");

        let store = aether_store::SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open graph");
        let sym_a = symbol("sym-a", "a", "src/a.rs");
        let sym_b = symbol("sym-b", "b", "src/b.rs");
        let sym_c = symbol("sym-c", "c", "src/c.rs");
        upsert_symbol_and_node(&store, &graph, &sym_a);
        upsert_symbol_and_node(&store, &graph, &sym_b);
        upsert_symbol_and_node(&store, &graph, &sym_c);
        upsert_edge(&graph, &sym_a.id, &sym_b.id, EdgeKind::Calls);
        upsert_edge(&graph, &sym_a.id, &sym_c.id, EdgeKind::Calls);

        let now = now_millis();
        record_history(
            &store,
            &sym_b.id,
            "hash-b1",
            r#"{"purpose":"old b","edge_cases":[],"dependencies":[]}"#,
            None,
            now.saturating_sub(2_000),
        );
        record_history(
            &store,
            &sym_b.id,
            "hash-b2",
            r#"{"purpose":"new b","edge_cases":["edge"],"dependencies":["db"]}"#,
            None,
            now.saturating_sub(1_000),
        );

        drop(graph);
        let analyzer = CausalAnalyzer::new(temp.path()).expect("new analyzer");
        let result = analyzer
            .trace_cause(TraceCauseRequest {
                target_symbol_id: sym_a.id.clone(),
                lookback: Some("7d".to_owned()),
                max_depth: Some(3),
                limit: Some(5),
            })
            .expect("trace cause");
        assert!(
            result
                .causal_chain
                .iter()
                .any(|entry| entry.symbol_id == sym_b.id)
        );
        assert!(result.skipped_missing_history >= 1);
        assert!(result.embedding_fallback_count >= 1);
    }

    fn init_repo(workspace: &Path, embeddings_enabled: bool) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["config", "user.email", "tester@example.com"]);
        run_git(workspace, &["config", "user.name", "Tester"]);
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = {embeddings_enabled}
provider = "mock"
vector_backend = "sqlite"

[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"
"#
            ),
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

    fn upsert_symbol_and_node(
        store: &aether_store::SqliteStore,
        graph: &CozoGraphStore,
        symbol: &SymbolRecord,
    ) {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        graph.upsert_symbol_node(symbol).expect("upsert node");
    }

    fn upsert_edge(graph: &CozoGraphStore, source_id: &str, target_id: &str, edge_kind: EdgeKind) {
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: source_id.to_owned(),
                target_id: target_id.to_owned(),
                edge_kind,
                file_path: "src/lib.rs".to_owned(),
            })
            .expect("upsert edge");
    }

    fn record_history(
        store: &aether_store::SqliteStore,
        symbol_id: &str,
        sir_hash: &str,
        sir_json: &str,
        commit_hash: Option<&str>,
        created_at: i64,
    ) {
        store
            .record_sir_version_if_changed(
                symbol_id,
                sir_hash,
                "mock",
                "mock",
                sir_json,
                created_at.max(0),
                commit_hash,
            )
            .expect("record history");
    }

    fn commit_marker(workspace: &Path, marker: &str) -> String {
        let path = workspace.join("markers").join(format!("{marker}.txt"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create marker parent");
        }
        fs::write(path, marker).expect("write marker");
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", marker]);
        git_output(workspace, &["rev-parse", "HEAD"])
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
