use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use aether_core::normalize_path;
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{
    CouplingEdgeRecord, GraphDependencyEdgeRecord, SemanticIndexStore, SqliteStore,
    SurrealGraphStore, SymbolCatalogStore, SymbolSearchResult, TaskContextHistoryRecord,
    block_on_store_future, open_surreal_graph_store_readonly,
};
use anyhow::{Context, Result, anyhow};
use gix::bstr::ByteSlice;
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::cli::{TaskHistoryArgs, TaskRelevanceArgs};
use crate::observer::is_ignored_path;

const DEFAULT_RRF_K: f64 = 60.0;
const DEFAULT_PPR_ALPHA: f64 = 0.15;
const DEFAULT_PPR_ITERATIONS: usize = 20;
const DEFAULT_BLEND_BETA: f64 = 0.6;
const DEFAULT_TOP_K_SEEDS: usize = 20;
const DEFAULT_RETRIEVAL_MULTIPLIER: usize = 3;
type DenseRetrievalResult = (Vec<(String, f32)>, Option<String>);

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "into", "that", "this", "these", "those", "then", "than",
    "when", "where", "while", "what", "which", "who", "why", "how", "are", "was", "were", "will",
    "would", "should", "could", "must", "have", "has", "had", "use", "using", "used", "need",
    "needs", "make", "makes", "made", "task", "context",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TaskSymbolResolution {
    pub ranked_symbols: Vec<(String, f64)>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BranchDiffResolution {
    symbol_ids: Vec<String>,
    file_paths: Vec<String>,
    notices: Vec<String>,
}

#[derive(Debug, Clone)]
struct SparseCandidate {
    qualified_name: String,
    file_path: String,
    matched_tokens: HashSet<String>,
    best_match_class: usize,
    best_rank: usize,
}

fn embedding_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("task-context tokio runtime should initialize")
    })
}

fn block_on_runtime<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|scope| {
            scope
                .spawn(|| embedding_runtime().block_on(future))
                .join()
                .expect("task-context runtime thread should not panic")
        })
    } else {
        embedding_runtime().block_on(future)
    }
}

pub fn dense_retrieval(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    limit: u32,
) -> Result<Vec<(String, f32)>> {
    Ok(dense_retrieval_with_notice(workspace, store, task_description, limit)?.0)
}

fn dense_retrieval_with_notice(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    limit: u32,
) -> Result<DenseRetrievalResult> {
    let task_description = task_description.trim();
    if task_description.is_empty() {
        return Ok((Vec::new(), None));
    }

    let loaded = match load_embedding_provider_from_config(
        workspace,
        EmbeddingProviderOverrides::default(),
    ) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            return Ok((
                Vec::new(),
                Some("embeddings are disabled in .aether/config.toml; running sparse-only task ranking".to_owned()),
            ));
        }
        Err(err) => {
            return Ok((
                Vec::new(),
                Some(format!(
                    "failed to load embedding provider for task ranking; running sparse-only: {err}"
                )),
            ));
        }
    };

    let embedding = match block_on_runtime(loaded.provider.embed_text(task_description)) {
        Ok(embedding) if !embedding.is_empty() => embedding,
        Ok(_) => {
            return Ok((
                Vec::new(),
                Some(
                    "embedding provider returned an empty vector for task ranking; running sparse-only"
                        .to_owned(),
                ),
            ));
        }
        Err(err) => {
            return Ok((
                Vec::new(),
                Some(format!(
                    "embedding generation failed for task ranking; running sparse-only: {err}"
                )),
            ));
        }
    };

    let results = store
        .search_symbols_semantic(
            embedding.as_slice(),
            loaded.provider_name.as_str(),
            loaded.model_name.as_str(),
            limit,
        )
        .context("failed to search symbols semantically for task ranking")?;
    Ok((
        results
            .into_iter()
            .map(|row| (row.symbol_id, row.semantic_score))
            .collect(),
        None,
    ))
}

pub fn sparse_retrieval(
    store: &SqliteStore,
    task_description: &str,
    limit: u32,
) -> Result<Vec<(String, usize)>> {
    let keywords = extract_keywords(task_description);
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let per_term_limit = limit.max(10);
    let mut candidates: HashMap<String, SparseCandidate> = HashMap::new();
    for token in &keywords {
        let matches = store
            .search_symbols(token.as_str(), per_term_limit)
            .with_context(|| format!("failed to search symbols for token '{token}'"))?;
        for (rank, candidate) in matches.into_iter().enumerate() {
            let match_class = classify_sparse_match(&candidate, token.as_str());
            let entry = candidates
                .entry(candidate.symbol_id.clone())
                .or_insert_with(|| SparseCandidate {
                    qualified_name: candidate.qualified_name.clone(),
                    file_path: candidate.file_path.clone(),
                    matched_tokens: HashSet::new(),
                    best_match_class: usize::MAX,
                    best_rank: usize::MAX,
                });
            entry.matched_tokens.insert(token.clone());
            entry.best_match_class = entry.best_match_class.min(match_class);
            entry.best_rank = entry.best_rank.min(rank + 1);
        }
    }

    let mut ranked = candidates.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_id, left), (right_id, right)| {
        right
            .matched_tokens
            .len()
            .cmp(&left.matched_tokens.len())
            .then_with(|| left.best_match_class.cmp(&right.best_match_class))
            .then_with(|| left.best_rank.cmp(&right.best_rank))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left_id.cmp(right_id))
    });

    Ok(ranked
        .into_iter()
        .take(limit as usize)
        .enumerate()
        .map(|(index, (symbol_id, _))| (symbol_id, index + 1))
        .collect())
}

pub fn reciprocal_rank_fusion(
    dense: &[(String, f32)],
    sparse: &[(String, usize)],
    k: f64,
) -> Vec<(String, f64)> {
    let mut scores = HashMap::<String, f64>::new();
    for (rank, (symbol_id, _)) in dense.iter().enumerate() {
        *scores.entry(symbol_id.clone()).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
    }
    for (symbol_id, rank) in sparse {
        *scores.entry(symbol_id.clone()).or_insert(0.0) += 1.0 / (k + *rank as f64);
    }

    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked
}

pub fn personalized_pagerank(
    graph: &DiGraph<String, ()>,
    node_map: &HashMap<String, NodeIndex>,
    seed_symbol_ids: &[String],
    alpha: f64,
    iterations: usize,
) -> HashMap<String, f64> {
    if graph.node_count() == 0 || seed_symbol_ids.is_empty() {
        return HashMap::new();
    }

    let nodes = graph.node_indices().collect::<Vec<_>>();
    let seed_nodes = seed_symbol_ids
        .iter()
        .filter_map(|symbol_id| node_map.get(symbol_id).copied())
        .collect::<Vec<_>>();
    if seed_nodes.is_empty() {
        return HashMap::new();
    }

    let seed_weight = 1.0 / seed_nodes.len() as f64;
    let mut seed_vector = HashMap::<NodeIndex, f64>::new();
    for node in seed_nodes {
        seed_vector.insert(node, seed_weight);
    }

    let out_degree = nodes
        .iter()
        .map(|node| {
            (
                *node,
                graph
                    .neighbors_directed(*node, Direction::Outgoing)
                    .count()
                    .max(1),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut rank = nodes
        .iter()
        .map(|node| (*node, seed_vector.get(node).copied().unwrap_or(0.0)))
        .collect::<HashMap<_, _>>();

    for _ in 0..iterations {
        let dangling_sum = nodes
            .iter()
            .filter(|node| {
                graph
                    .neighbors_directed(**node, Direction::Outgoing)
                    .next()
                    .is_none()
            })
            .map(|node| rank.get(node).copied().unwrap_or(0.0))
            .sum::<f64>();

        let mut delta = 0.0;
        let mut next = HashMap::<NodeIndex, f64>::new();
        for node in &nodes {
            let incoming_sum = graph
                .neighbors_directed(*node, Direction::Incoming)
                .map(|incoming| {
                    rank.get(&incoming).copied().unwrap_or(0.0)
                        / out_degree.get(&incoming).copied().unwrap_or(1) as f64
                })
                .sum::<f64>();
            let seed_mass = seed_vector.get(node).copied().unwrap_or(0.0);
            let next_score =
                (1.0 - alpha) * (incoming_sum + dangling_sum * seed_mass) + alpha * seed_mass;
            delta += (next_score - rank.get(node).copied().unwrap_or(0.0)).abs();
            next.insert(*node, next_score);
        }
        rank = next;
        if delta < 1e-8 {
            break;
        }
    }

    rank.into_iter()
        .filter_map(|(node, score)| {
            graph
                .node_weight(node)
                .cloned()
                .filter(|_| score > 0.0)
                .map(|symbol_id| (symbol_id, score))
        })
        .collect()
}

pub fn branch_diff_to_symbols(
    workspace: &Path,
    store: &SqliteStore,
    branch_name: &str,
) -> Result<Vec<String>> {
    Ok(branch_diff_to_symbols_with_context(workspace, store, branch_name)?.symbol_ids)
}

fn branch_diff_to_symbols_with_context(
    workspace: &Path,
    store: &SqliteStore,
    branch_name: &str,
) -> Result<BranchDiffResolution> {
    let branch_name = branch_name.trim();
    if branch_name.is_empty() {
        return Err(anyhow!("branch name must not be empty"));
    }

    let changed_paths = changed_paths_between_refs(workspace, "main", branch_name)?;
    let mut file_paths = changed_paths.into_iter().collect::<BTreeSet<_>>();
    let mut notices = Vec::new();
    let graph_store = {
        let skip = aether_config::load_workspace_config(workspace)
            .ok()
            .and_then(|cfg| {
                if matches!(
                    cfg.storage.graph_backend,
                    aether_config::GraphBackend::Surreal | aether_config::GraphBackend::Cozo
                ) {
                    crate::daemon_detect::detect_running_daemon(&cfg, workspace)
                } else {
                    None
                }
            });
        if let Some(ref daemon) = skip {
            crate::daemon_detect::warn_daemon_detected(daemon, "task-context");
            None
        } else {
            open_surreal_graph_store_readonly(workspace).ok()
        }
    };

    match coupled_file_paths(
        graph_store.as_ref(),
        file_paths.iter().cloned().collect::<Vec<_>>().as_slice(),
    ) {
        Ok(paths) => {
            for path in paths {
                file_paths.insert(path);
            }
        }
        Err(notice) => notices.push(notice),
    }

    let mut symbol_ids = BTreeSet::new();
    for file_path in &file_paths {
        let records = store
            .list_symbols_for_file(file_path.as_str())
            .with_context(|| format!("failed to list symbols for changed file {file_path}"))?;
        for record in records {
            symbol_ids.insert(record.id);
        }
    }
    if !file_paths.is_empty() && symbol_ids.is_empty() {
        notices.push(format!(
            "branch diff matched {} file(s) but no indexed symbols",
            file_paths.len()
        ));
    }

    Ok(BranchDiffResolution {
        symbol_ids: symbol_ids.into_iter().collect(),
        file_paths: file_paths.into_iter().collect(),
        notices,
    })
}

pub fn resolve_task_symbols(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    branch: Option<&str>,
    top_k_seeds: usize,
    beta: f64,
) -> Result<Vec<(String, f64)>> {
    Ok(resolve_task_symbols_with_context(
        workspace,
        store,
        task_description,
        branch,
        top_k_seeds,
        beta,
    )?
    .ranked_symbols)
}

pub(crate) fn resolve_task_symbols_with_context(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    branch: Option<&str>,
    top_k_seeds: usize,
    beta: f64,
) -> Result<TaskSymbolResolution> {
    let task_description = task_description.trim();
    if task_description.is_empty() {
        return Err(anyhow!("task description must not be empty"));
    }

    let retrieval_limit = (top_k_seeds.max(DEFAULT_TOP_K_SEEDS) * DEFAULT_RETRIEVAL_MULTIPLIER)
        .clamp(DEFAULT_TOP_K_SEEDS, 100) as u32;
    let (dense, dense_notice) =
        dense_retrieval_with_notice(workspace, store, task_description, retrieval_limit)?;
    let sparse = sparse_retrieval(store, task_description, retrieval_limit)?;
    let rrf = reciprocal_rank_fusion(&dense, &sparse, DEFAULT_RRF_K);

    let mut notices = Vec::new();
    if let Some(notice) = dense_notice {
        notices.push(notice);
    }

    let mut seed_symbol_ids = rrf
        .iter()
        .take(top_k_seeds.max(1))
        .map(|(symbol_id, _)| symbol_id.clone())
        .collect::<Vec<_>>();

    if let Some(branch_name) = branch {
        let branch_resolution = branch_diff_to_symbols_with_context(workspace, store, branch_name)?;
        notices.extend(branch_resolution.notices);
        seed_symbol_ids.extend(branch_resolution.symbol_ids);
    }

    let mut seen = HashSet::new();
    seed_symbol_ids.retain(|symbol_id| seen.insert(symbol_id.clone()));
    if seed_symbol_ids.is_empty() {
        notices.push("task context resolved no seed symbols".to_owned());
        return Ok(TaskSymbolResolution {
            ranked_symbols: Vec::new(),
            notices,
        });
    }

    let graph_edges = store
        .list_graph_dependency_edges()
        .context("failed to load dependency graph edges for task ranking")?;
    let (mut graph, mut node_map) = load_dependency_graph(&graph_edges);
    for symbol_id in &seed_symbol_ids {
        if !node_map.contains_key(symbol_id) {
            let index = graph.add_node(symbol_id.clone());
            node_map.insert(symbol_id.clone(), index);
        }
    }

    let ppr = personalized_pagerank(
        &graph,
        &node_map,
        seed_symbol_ids.as_slice(),
        DEFAULT_PPR_ALPHA,
        DEFAULT_PPR_ITERATIONS,
    );

    let clamped_beta = beta.clamp(0.0, 1.0);
    let mut final_scores = HashMap::<String, f64>::new();
    for (symbol_id, score) in &rrf {
        *final_scores.entry(symbol_id.clone()).or_insert(0.0) += clamped_beta * *score;
    }
    for (symbol_id, score) in ppr {
        *final_scores.entry(symbol_id).or_insert(0.0) += (1.0 - clamped_beta) * score;
    }

    let mut ranked_symbols = final_scores
        .into_iter()
        .filter(|(_, score)| *score > 0.0)
        .collect::<Vec<_>>();
    ranked_symbols.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    Ok(TaskSymbolResolution {
        ranked_symbols,
        notices,
    })
}

pub fn run_task_history_command(workspace: &Path, args: TaskHistoryArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let history = store
        .list_recent_task_history(args.limit)
        .context("failed to list recent task history")?;
    let mut out = std::io::stdout();
    write_task_history(&history, &mut out)
}

pub fn run_task_relevance_command(workspace: &Path, args: TaskRelevanceArgs) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let resolution = resolve_task_symbols_with_context(
        workspace,
        &store,
        args.task.as_str(),
        args.branch.as_deref(),
        DEFAULT_TOP_K_SEEDS,
        DEFAULT_BLEND_BETA,
    )?;
    let top = args.top.max(1);
    let top_ids = resolution
        .ranked_symbols
        .iter()
        .take(top)
        .map(|(symbol_id, _)| symbol_id.clone())
        .collect::<Vec<_>>();
    let rows = store
        .get_symbol_search_results_batch(top_ids.as_slice())
        .context("failed to resolve ranked symbols for task relevance output")?;

    let mut out = std::io::stdout();
    write_task_relevance(
        &resolution.notices,
        &resolution.ranked_symbols,
        &rows,
        top,
        &mut out,
    )
}

fn write_task_history(history: &[TaskContextHistoryRecord], out: &mut dyn Write) -> Result<()> {
    if history.is_empty() {
        writeln!(out, "no history").context("failed to write empty task history output")?;
        return Ok(());
    }

    writeln!(out, "created_at\tbranch\tsymbols\tbudget\ttask")
        .context("failed to write task history header")?;
    for entry in history {
        writeln!(
            out,
            "{}\t{}\t{}\t{}/{}\t{}",
            entry.created_at,
            entry.branch_name.as_deref().unwrap_or("-"),
            entry.total_symbols,
            entry.budget_used,
            entry.budget_max,
            entry.task_description
        )
        .context("failed to write task history row")?;
    }
    Ok(())
}

fn write_task_relevance(
    notices: &[String],
    ranked_symbols: &[(String, f64)],
    rows: &HashMap<String, SymbolSearchResult>,
    top: usize,
    out: &mut dyn Write,
) -> Result<()> {
    for notice in notices {
        writeln!(out, "warning: {notice}").context("failed to write task relevance notice")?;
    }

    if ranked_symbols.is_empty() {
        writeln!(out, "no ranked symbols")
            .context("failed to write empty task relevance output")?;
        return Ok(());
    }

    writeln!(out, "score\tqualified_name\tfile_path\tsymbol_id")
        .context("failed to write task relevance header")?;
    for (symbol_id, score) in ranked_symbols.iter().take(top) {
        if let Some(row) = rows.get(symbol_id) {
            writeln!(
                out,
                "{score:.6}\t{}\t{}\t{}",
                row.qualified_name, row.file_path, row.symbol_id
            )
            .context("failed to write task relevance row")?;
        } else {
            writeln!(out, "{score:.6}\t<missing>\t<missing>\t{symbol_id}")
                .context("failed to write task relevance fallback row")?;
        }
    }
    Ok(())
}

fn extract_keywords(task_description: &str) -> Vec<String> {
    let mut keywords = Vec::new();
    let mut seen = HashSet::new();
    for token in task_description
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3)
        .filter(|token| !STOPWORDS.iter().any(|stopword| stopword == token))
    {
        if seen.insert(token.clone()) {
            keywords.push(token);
        }
        if keywords.len() >= 8 {
            break;
        }
    }
    keywords
}

fn classify_sparse_match(candidate: &SymbolSearchResult, token: &str) -> usize {
    let token = token.trim().to_ascii_lowercase();
    let qualified_name = candidate.qualified_name.to_ascii_lowercase();
    let file_path = candidate.file_path.to_ascii_lowercase();
    let leaf = leaf_name(candidate.qualified_name.as_str()).to_ascii_lowercase();
    if leaf == token {
        return 0;
    }
    if split_qualified_name(candidate.qualified_name.as_str())
        .into_iter()
        .any(|segment| segment.starts_with(token.as_str()))
    {
        return 1;
    }
    if qualified_name.contains(token.as_str()) {
        return 2;
    }
    if file_path.contains(token.as_str()) {
        return 3;
    }
    if candidate.language.eq_ignore_ascii_case(token.as_str())
        || candidate.kind.eq_ignore_ascii_case(token.as_str())
    {
        return 4;
    }
    5
}

fn leaf_name(qualified_name: &str) -> &str {
    qualified_name
        .rsplit("::")
        .next()
        .or_else(|| qualified_name.rsplit('.').next())
        .unwrap_or(qualified_name)
}

fn split_qualified_name(qualified_name: &str) -> Vec<String> {
    qualified_name
        .replace("::", ".")
        .split('.')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_ascii_lowercase())
        .collect()
}

fn changed_paths_between_refs(
    workspace: &Path,
    old_ref: &str,
    new_ref: &str,
) -> Result<Vec<String>> {
    let repo = gix::discover(workspace).context("failed to open git repo for task branch diff")?;
    let old_id = repo
        .rev_parse_single(old_ref)
        .with_context(|| format!("failed to resolve git ref {old_ref}"))?;
    let new_id = repo
        .rev_parse_single(new_ref)
        .with_context(|| format!("failed to resolve git ref {new_ref}"))?;
    let old_commit = repo
        .find_commit(old_id.detach())
        .with_context(|| format!("failed to load commit {old_ref}"))?;
    let new_commit = repo
        .find_commit(new_id.detach())
        .with_context(|| format!("failed to load commit {new_ref}"))?;
    let old_tree = old_commit
        .tree()
        .with_context(|| format!("failed to load tree for {old_ref}"))?;
    let new_tree = new_commit
        .tree()
        .with_context(|| format!("failed to load tree for {new_ref}"))?;

    let mut diff_options = gix::diff::Options::default();
    diff_options.track_rewrites(None);
    let changes = repo
        .diff_tree_to_tree(Some(&old_tree), Some(&new_tree), Some(diff_options))
        .with_context(|| format!("failed to diff git refs {old_ref}..{new_ref}"))?;

    let mut paths = BTreeSet::new();
    for change in changes {
        let raw_path = change.location().to_str_lossy();
        let normalized = normalize_path(normalize_git_rename_path(raw_path.as_ref()).as_str());
        if normalized.is_empty() || is_ignored_path(Path::new(&normalized)) {
            continue;
        }
        paths.insert(normalized);
    }
    Ok(paths.into_iter().collect())
}

fn normalize_git_rename_path(path: &str) -> String {
    let value = path.trim();
    if let (Some(brace_start), Some(brace_end)) = (value.find('{'), value.find('}'))
        && brace_start < brace_end
    {
        let prefix = &value[..brace_start];
        let inner = &value[brace_start + 1..brace_end];
        let suffix = &value[brace_end + 1..];
        if let Some((_, new_part)) = inner.split_once("=>") {
            return format!("{}{}{}", prefix, new_part.trim(), suffix);
        }
    }
    if let Some((_, right)) = value.rsplit_once("=>") {
        return right.trim().to_owned();
    }
    value.to_owned()
}

fn coupled_file_paths(
    graph_store: Option<&SurrealGraphStore>,
    file_paths: &[String],
) -> std::result::Result<Vec<String>, String> {
    if file_paths.is_empty() {
        return Ok(Vec::new());
    }

    let Some(graph_store) = graph_store else {
        return Err("coupling data unavailable — daemon may hold SurrealDB lock".to_owned());
    };

    let mut coupled = BTreeSet::new();
    for file_path in file_paths {
        let edges = match block_on_store_future(
            graph_store.list_co_change_edges_for_file(file_path.as_str(), 0.0),
        ) {
            Ok(Ok(edges)) => edges,
            Err(_) => {
                return Err("coupling data unavailable — daemon may hold SurrealDB lock".to_owned());
            }
            Ok(Err(_)) => {
                return Err("coupling data unavailable — daemon may hold SurrealDB lock".to_owned());
            }
        };
        for edge in edges {
            if let Some(other) = other_file_from_coupling(file_path.as_str(), &edge)
                && !other.is_empty()
                && !is_ignored_path(Path::new(other.as_str()))
            {
                coupled.insert(other);
            }
        }
    }
    Ok(coupled.into_iter().collect())
}

fn other_file_from_coupling(file_path: &str, edge: &CouplingEdgeRecord) -> Option<String> {
    let normalized = normalize_path(file_path);
    let file_a = normalize_path(edge.file_a.as_str());
    let file_b = normalize_path(edge.file_b.as_str());
    if file_a == normalized {
        Some(file_b)
    } else if file_b == normalized {
        Some(file_a)
    } else {
        None
    }
}

fn load_dependency_graph(
    edges: &[GraphDependencyEdgeRecord],
) -> (DiGraph<String, ()>, HashMap<String, NodeIndex>) {
    let mut graph = DiGraph::new();
    let mut node_map = HashMap::new();
    for edge in edges {
        let source_idx = *node_map
            .entry(edge.source_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.source_symbol_id.clone()));
        let target_idx = *node_map
            .entry(edge.target_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.target_symbol_id.clone()));
        graph.add_edge(source_idx, target_idx, ());
    }
    (graph, node_map)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use aether_store::{SymbolRecord, TaskContextHistoryRecord};
    use tempfile::tempdir;

    use super::*;

    fn write_test_config(workspace: &Path) {
        std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        std::fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"

[storage]
graph_backend = "sqlite"
mirror_sir_files = true

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn upsert_symbol(store: &SqliteStore, id: &str, file_path: &str, qualified_name: &str) {
        store
            .upsert_symbol(SymbolRecord {
                id: id.to_owned(),
                file_path: file_path.to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: qualified_name.to_owned(),
                signature_fingerprint: format!("sig-{id}"),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
    }

    fn init_git_repo(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["branch", "-M", "main"]);
        run_git(workspace, &["config", "user.name", "Aether Test"]);
        run_git(
            workspace,
            &["config", "user.email", "aether-test@example.com"],
        );
    }

    fn run_git(workspace: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("run git");
        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout)
            .expect("git stdout utf8")
            .trim()
            .to_owned()
    }

    #[test]
    fn rrf_scores_symbols_from_single_and_dual_sources() {
        let dense = vec![
            ("sym-a".to_owned(), 0.91),
            ("sym-b".to_owned(), 0.80),
            ("sym-c".to_owned(), 0.75),
        ];
        let sparse = vec![("sym-b".to_owned(), 1), ("sym-d".to_owned(), 2)];

        let ranked = reciprocal_rank_fusion(&dense, &sparse, 60.0);
        assert_eq!(ranked[0].0, "sym-b");
        assert!(ranked.iter().any(|(symbol_id, _)| symbol_id == "sym-a"));
        assert!(ranked.iter().any(|(symbol_id, _)| symbol_id == "sym-d"));
    }

    #[test]
    fn personalized_pagerank_prefers_seed_and_decays_with_distance() {
        let mut graph = DiGraph::<String, ()>::new();
        let seed = graph.add_node("sym-seed".to_owned());
        let near = graph.add_node("sym-near".to_owned());
        let far = graph.add_node("sym-far".to_owned());
        graph.add_edge(seed, near, ());
        graph.add_edge(near, far, ());

        let node_map = HashMap::from([
            ("sym-seed".to_owned(), seed),
            ("sym-near".to_owned(), near),
            ("sym-far".to_owned(), far),
        ]);

        let ranked = personalized_pagerank(&graph, &node_map, &["sym-seed".to_owned()], 0.15, 20);
        assert!(ranked["sym-seed"] > ranked["sym-near"]);
        assert!(ranked["sym-near"] > ranked["sym-far"]);
    }

    #[test]
    fn branch_diff_maps_changed_files_to_symbols() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        init_git_repo(workspace);
        std::fs::create_dir_all(workspace.join("src")).expect("create src");
        std::fs::write(workspace.join("src/lib.rs"), "fn alpha() -> i32 { 1 }\n")
            .expect("write lib");
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", "main"]);
        run_git(workspace, &["checkout", "-b", "feature/fix-auth"]);
        std::fs::write(workspace.join("src/lib.rs"), "fn alpha() -> i32 { 2 }\n")
            .expect("update lib");
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", "feature"]);

        let store = SqliteStore::open(workspace).expect("open store");
        upsert_symbol(&store, "sym-alpha", "src/lib.rs", "demo::alpha");
        upsert_symbol(&store, "sym-beta", "src/other.rs", "demo::beta");

        let changed_symbols = branch_diff_to_symbols(workspace, &store, "feature/fix-auth")
            .expect("branch diff symbols");
        assert_eq!(changed_symbols, vec!["sym-alpha".to_owned()]);
    }

    #[test]
    fn sparse_only_resolution_works_when_embeddings_are_disabled() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        upsert_symbol(&store, "sym-auth", "src/auth.rs", "auth::repair_flow");
        upsert_symbol(&store, "sym-cache", "src/cache.rs", "cache::refresh");

        let resolution =
            resolve_task_symbols_with_context(workspace, &store, "repair auth flow", None, 20, 0.6)
                .expect("resolve task symbols");
        assert!(!resolution.ranked_symbols.is_empty());
        assert!(
            resolution
                .notices
                .iter()
                .any(|notice| notice.contains("sparse-only"))
        );
        assert_eq!(resolution.ranked_symbols[0].0, "sym-auth");
    }

    #[test]
    fn task_history_command_initializes_store_on_fresh_workspace() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        run_task_history_command(workspace, TaskHistoryArgs { limit: 10 })
            .expect("run task-history");

        assert!(workspace.join(".aether").join("meta.sqlite").exists());
        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let history = reopened
            .list_recent_task_history(10)
            .expect("list recent task history");
        assert!(history.is_empty());
    }

    #[test]
    fn task_relevance_command_initializes_store_on_fresh_workspace() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        run_task_relevance_command(
            workspace,
            TaskRelevanceArgs {
                task: "repair auth flow".to_owned(),
                branch: None,
                top: 5,
            },
        )
        .expect("run task-relevance");

        assert!(workspace.join(".aether").join("meta.sqlite").exists());
    }

    #[test]
    fn task_history_writer_reports_empty_and_rows() {
        let mut out = Vec::new();
        write_task_history(&[], &mut out).expect("write empty history");
        assert_eq!(String::from_utf8(out).expect("utf8"), "no history\n");

        let mut out = Vec::new();
        write_task_history(
            &[TaskContextHistoryRecord {
                task_description: "repair auth flow".to_owned(),
                branch_name: Some("feature/fix-auth".to_owned()),
                resolved_symbol_ids: "[]".to_owned(),
                resolved_file_paths: "[]".to_owned(),
                total_symbols: 2,
                budget_used: 1200,
                budget_max: 32000,
                created_at: 1_700_000_000,
            }],
            &mut out,
        )
        .expect("write history rows");
        let rendered = String::from_utf8(out).expect("utf8");
        assert!(rendered.contains("created_at\tbranch\tsymbols\tbudget\ttask"));
        assert!(rendered.contains("repair auth flow"));
    }
}
