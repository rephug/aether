//! Priority scores for SIR generation: PageRank over the dependency graph and git recency.

use super::*;

pub fn compute_symbol_priority_scores(
    workspace: &Path,
    store: &SqliteStore,
    symbols: &[Symbol],
) -> HashMap<String, f64> {
    let file_paths = symbols
        .iter()
        .map(|symbol| symbol.file_path.clone())
        .collect::<HashSet<_>>();
    let git_scores = collect_git_recency_scores(workspace, &file_paths);
    let page_rank_scores = collect_page_rank_scores(store);

    let mut source_cache = HashMap::<String, String>::new();
    let mut line_count_cache = HashMap::<String, usize>::new();
    let mut scores = HashMap::new();
    for symbol in symbols {
        let git_recency = git_scores
            .get(symbol.file_path.as_str())
            .copied()
            .unwrap_or(0.0);
        let page_rank = page_rank_scores
            .get(symbol.id.as_str())
            .copied()
            .unwrap_or(0.0);
        let line_count = *line_count_cache
            .entry(symbol.file_path.clone())
            .or_insert_with(|| source_line_count(workspace, symbol.file_path.as_str()));
        let source = source_cache
            .entry(symbol.file_path.clone())
            .or_insert_with(|| read_source_file(workspace, symbol.file_path.as_str()));
        let is_public = infer_symbol_is_public(source.as_str(), symbol);
        let kind_score = kind_priority_score(symbol.kind.as_str(), is_public);
        let size_score = size_inverse_score(line_count);
        let score = compute_priority_score(git_recency, page_rank, kind_score, size_score);
        scores.insert(symbol.id.clone(), score);
    }

    scores
}

pub(super) fn collect_page_rank_scores(store: &SqliteStore) -> HashMap<String, f64> {
    let Ok(edges) = store.list_graph_dependency_edges() else {
        return HashMap::new();
    };
    if edges.is_empty() {
        return HashMap::new();
    }

    let algo_edges = edges
        .into_iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_symbol_id,
            target_id: edge.target_symbol_id,
            edge_kind: edge.edge_kind,
        })
        .collect::<Vec<_>>();

    let ranked = page_rank_sync(&algo_edges, 0.85, 20);
    let max_score = ranked
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0_f64, f64::max);
    if max_score <= f64::EPSILON {
        return ranked
            .into_iter()
            .map(|(symbol_id, _)| (symbol_id, 0.0))
            .collect();
    }

    ranked
        .into_iter()
        .map(|(symbol_id, score)| (symbol_id, (score / max_score).clamp(0.0, 1.0)))
        .collect()
}

pub(super) fn collect_git_recency_scores(
    workspace: &Path,
    file_paths: &HashSet<String>,
) -> HashMap<String, f64> {
    let Some(context) = GitContext::open(workspace) else {
        return HashMap::new();
    };
    let recent_commit_positions = recent_commit_positions(workspace, 10);
    if recent_commit_positions.is_empty() {
        return HashMap::new();
    }

    let mut scores = HashMap::new();
    for file_path in file_paths {
        let history = context.file_log(Path::new(file_path), 128);
        let mut score = 0.0_f64;
        for commit in history {
            if let Some(position) = recent_commit_positions.get(commit.hash.as_str()) {
                score = (1.0 - (*position as f64 / 10.0)).clamp(0.0, 1.0);
                break;
            }
        }
        scores.insert(file_path.clone(), score);
    }

    scores
}

pub(super) fn recent_commit_positions(workspace: &Path, limit: usize) -> HashMap<String, usize> {
    let mut positions = HashMap::new();
    if limit == 0 {
        return positions;
    }

    let Ok(repo) = gix::discover(workspace) else {
        return positions;
    };
    let Some(head_id) = repo.head_id().ok().map(|id| id.detach()) else {
        return positions;
    };
    let Ok(walk) = repo
        .rev_walk([head_id])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ))
        .all()
    else {
        return positions;
    };

    for (position, entry) in walk.flatten().take(limit).enumerate() {
        positions.insert(entry.id.to_string().to_ascii_lowercase(), position);
    }
    positions
}
