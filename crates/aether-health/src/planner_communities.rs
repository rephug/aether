use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use aether_core::SymbolKind;
use aether_graph_algo::{GraphAlgorithmEdge, louvain_with_resolution_sync};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct FileCommunityConfig {
    pub semantic_rescue_threshold: f32,
    pub semantic_rescue_max_k: usize,
    pub community_resolution: f64,
    pub min_community_size: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileSymbol {
    pub symbol_id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub is_test: bool,
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerDiagnostics {
    pub symbols_total: usize,
    pub symbols_filtered_test: usize,
    pub symbols_anchored_type: usize,
    pub symbols_rescued_container: usize,
    pub symbols_rescued_semantic: usize,
    pub symbols_loner: usize,
    pub communities_before_merge: usize,
    pub communities_after_merge: usize,
    pub embedding_coverage_pct: f32,
    pub confidence: f32,
    pub confidence_label: String,
    pub stability_score: f32,
}

#[derive(Clone)]
struct SymbolEntry {
    symbol: FileSymbol,
    stem: String,
}

#[derive(Clone, Default)]
struct WeightedGraph {
    edges: BTreeMap<(usize, usize), usize>,
}

#[derive(Clone)]
struct DetectionRun {
    assignments: Vec<(String, usize)>,
    symbols_total: usize,
    symbols_filtered_test: usize,
    symbols_anchored_type: usize,
    symbols_rescued_container: usize,
    symbols_rescued_semantic: usize,
    symbols_loner: usize,
    communities_before_merge: usize,
    communities_after_merge: usize,
    embedding_coverage_pct: f32,
    non_test_count: usize,
    unmerged_small_penalty: f32,
}

#[derive(Clone)]
struct CommunityState {
    reps: Vec<usize>,
    symbol_indices: Vec<usize>,
    symbol_count: usize,
    component_id: usize,
}

#[derive(Clone)]
struct MergeCandidate {
    community_id: usize,
    structural_edges: usize,
    semantic_affinity: Option<f32>,
    size: usize,
}

#[derive(Clone)]
struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, value: usize) -> usize {
        if self.parent.get(value).copied() == Some(value) {
            return value;
        }
        let parent = self.parent.get(value).copied().unwrap_or(value);
        let root = self.find(parent);
        if let Some(slot) = self.parent.get_mut(value) {
            *slot = root;
        }
        root
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root == right_root {
            return;
        }

        let left_rank = self.rank.get(left_root).copied().unwrap_or(0);
        let right_rank = self.rank.get(right_root).copied().unwrap_or(0);
        if left_rank < right_rank {
            if let Some(slot) = self.parent.get_mut(left_root) {
                *slot = right_root;
            }
            return;
        }
        if left_rank > right_rank {
            if let Some(slot) = self.parent.get_mut(right_root) {
                *slot = left_root;
            }
            return;
        }

        if let Some(slot) = self.parent.get_mut(right_root) {
            *slot = left_root;
        }
        if let Some(slot) = self.rank.get_mut(left_root) {
            *slot += 1;
        }
    }
}

impl WeightedGraph {
    fn add_edge(&mut self, left: usize, right: usize, weight: usize) {
        if left == right || weight == 0 {
            return;
        }
        let key = normalized_pair(left, right);
        *self.edges.entry(key).or_default() += weight;
    }

    fn degree(&self, node: usize) -> usize {
        self.edges
            .iter()
            .filter_map(|((left, right), weight)| {
                if *left == node || *right == node {
                    Some(*weight)
                } else {
                    None
                }
            })
            .sum()
    }

    fn neighbors(&self, node: usize) -> Vec<(usize, usize)> {
        let mut neighbors = self
            .edges
            .iter()
            .filter_map(|((left, right), weight)| {
                if *left == node {
                    Some((*right, *weight))
                } else if *right == node {
                    Some((*left, *weight))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        neighbors.sort_by(|left, right| left.0.cmp(&right.0));
        neighbors
    }

    fn connected_components(&self, nodes: &[usize], entries: &[SymbolEntry]) -> Vec<Vec<usize>> {
        let mut remaining = nodes.iter().copied().collect::<BTreeSet<_>>();
        let mut components = Vec::new();

        while let Some(start) = remaining.pop_first() {
            let mut queue = VecDeque::from([start]);
            let mut component = Vec::new();
            let mut seen = HashSet::from([start]);

            while let Some(node) = queue.pop_front() {
                component.push(node);
                for (neighbor, _) in self.neighbors(node) {
                    if !seen.insert(neighbor) {
                        continue;
                    }
                    remaining.remove(&neighbor);
                    queue.push_back(neighbor);
                }
            }

            component.sort_by(|left, right| {
                min_symbol_id_for_rep(*left, entries)
                    .cmp(&min_symbol_id_for_rep(*right, entries))
                    .then_with(|| left.cmp(right))
            });
            components.push(component);
        }

        components.sort_by(|left, right| {
            min_symbol_id_for_component(left, entries)
                .cmp(&min_symbol_id_for_component(right, entries))
                .then_with(|| left.len().cmp(&right.len()))
        });
        components
    }

    fn repeated_component_edges(&self, component: &[usize]) -> Vec<GraphAlgorithmEdge> {
        let component_set = component.iter().copied().collect::<HashSet<_>>();
        let mut edges = Vec::new();
        for ((left, right), weight) in &self.edges {
            if !component_set.contains(left) || !component_set.contains(right) {
                continue;
            }
            for _ in 0..*weight {
                edges.push(GraphAlgorithmEdge {
                    source_id: format!("rep-{left}"),
                    target_id: format!("rep-{right}"),
                    edge_kind: "calls".to_owned(),
                });
            }
        }
        edges.sort_by(|left, right| {
            left.source_id
                .cmp(&right.source_id)
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.edge_kind.cmp(&right.edge_kind))
        });
        edges
    }
}

pub fn detect_file_communities(
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
) -> (Vec<(String, usize)>, PlannerDiagnostics) {
    let baseline = run_detection(structural_edges, symbols, config);
    if baseline.non_test_count == 0 || baseline.assignments.is_empty() {
        let diagnostics = diagnostics_from_run(&baseline, 0.0, 0.0);
        return (Vec::new(), diagnostics);
    }

    let mut threshold_config = config.clone();
    threshold_config.semantic_rescue_threshold =
        (threshold_config.semantic_rescue_threshold + 0.05).clamp(0.3, 0.95);
    let threshold_run = run_detection(structural_edges, symbols, &threshold_config);

    let mut resolution_config = config.clone();
    resolution_config.community_resolution =
        (resolution_config.community_resolution + 0.1).clamp(0.1, 3.0);
    let resolution_run = run_detection(structural_edges, symbols, &resolution_config);

    let stability_score = pairwise_jaccard(
        baseline.assignments.as_slice(),
        threshold_run.assignments.as_slice(),
    )
    .min(pairwise_jaccard(
        baseline.assignments.as_slice(),
        resolution_run.assignments.as_slice(),
    ));

    let confidence = compute_confidence(&baseline, stability_score);
    let diagnostics = diagnostics_from_run(&baseline, stability_score, confidence);
    (baseline.assignments, diagnostics)
}

fn run_detection(
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
) -> DetectionRun {
    let symbols_total = symbols.len();
    let mut filtered_symbols = symbols
        .iter()
        .filter(|symbol| !symbol.is_test)
        .cloned()
        .collect::<Vec<_>>();
    filtered_symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
    let symbols_filtered_test = symbols_total.saturating_sub(filtered_symbols.len());
    let non_test_count = filtered_symbols.len();

    let entries = filtered_symbols
        .into_iter()
        .map(|symbol| SymbolEntry {
            stem: qualified_name_stem(symbol.qualified_name.as_str()),
            symbol,
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return DetectionRun {
            assignments: Vec::new(),
            symbols_total,
            symbols_filtered_test,
            symbols_anchored_type: 0,
            symbols_rescued_container: 0,
            symbols_rescued_semantic: 0,
            symbols_loner: 0,
            communities_before_merge: 0,
            communities_after_merge: 0,
            embedding_coverage_pct: 0.0,
            non_test_count: 0,
            unmerged_small_penalty: 0.0,
        };
    }

    let id_to_index = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.symbol.symbol_id.clone(), index))
        .collect::<HashMap<_, _>>();
    let filtered_structural_edges = structural_edges
        .iter()
        .filter(|edge| {
            id_to_index.contains_key(edge.source_id.as_str())
                && id_to_index.contains_key(edge.target_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();

    let (_anchor_union_find, anchor_groups) = build_anchor_groups(entries.as_slice());
    let (anchor_groups, split_anchor_exclusions) =
        split_large_anchor_groups(entries.as_slice(), anchor_groups);
    let symbols_anchored_type = count_type_anchored_symbols(entries.as_slice(), &anchor_groups);
    let mut union_find = rebuild_union_find_from_groups(entries.len(), anchor_groups.as_slice());

    let structural_graph = collapse_structural_edges(
        filtered_structural_edges.as_slice(),
        &id_to_index,
        &mut union_find,
    );
    let mut enriched_graph = structural_graph.clone();

    let rep_to_members = build_rep_members(entries.len(), &mut union_find);
    let rep_by_index = (0..entries.len())
        .map(|index| union_find.find(index))
        .collect::<Vec<_>>();

    let symbols_rescued_container = apply_container_rescue_with_exclusions(
        entries.as_slice(),
        rep_by_index.as_slice(),
        rep_to_members.as_slice(),
        &mut enriched_graph,
        &split_anchor_exclusions,
    );
    let symbols_rescued_semantic = apply_semantic_rescue(
        entries.as_slice(),
        rep_by_index.as_slice(),
        &mut enriched_graph,
        config,
    );

    let loner_reps = rep_to_members
        .iter()
        .enumerate()
        .filter_map(|(rep, members)| {
            if members.is_empty() || enriched_graph.degree(rep) > 0 {
                None
            } else {
                Some(rep)
            }
        })
        .collect::<HashSet<_>>();
    let symbols_loner = loner_reps
        .iter()
        .filter_map(|rep| rep_to_members.get(*rep))
        .map(Vec::len)
        .sum();
    let embedding_coverage_pct = if non_test_count == 0 {
        0.0
    } else {
        entries
            .iter()
            .filter(|entry| has_embedding(entry.symbol.embedding.as_deref()))
            .count() as f32
            / non_test_count as f32
    };

    let active_reps = rep_to_members
        .iter()
        .enumerate()
        .filter_map(|(rep, members)| {
            if members.is_empty() || loner_reps.contains(&rep) {
                None
            } else {
                Some(rep)
            }
        })
        .collect::<Vec<_>>();
    if active_reps.is_empty() {
        return DetectionRun {
            assignments: Vec::new(),
            symbols_total,
            symbols_filtered_test,
            symbols_anchored_type,
            symbols_rescued_container,
            symbols_rescued_semantic,
            symbols_loner,
            communities_before_merge: 0,
            communities_after_merge: 0,
            embedding_coverage_pct,
            non_test_count,
            unmerged_small_penalty: 0.0,
        };
    }

    let components =
        enriched_graph.connected_components(active_reps.as_slice(), entries.as_slice());
    let component_of_rep = components
        .iter()
        .enumerate()
        .flat_map(|(component_id, reps)| reps.iter().copied().map(move |rep| (rep, component_id)))
        .collect::<HashMap<_, _>>();
    let mut rep_to_community = HashMap::<usize, usize>::new();
    let mut next_community_id = 1usize;
    for component in &components {
        let local_edges = enriched_graph.repeated_component_edges(component.as_slice());
        let local_assignments =
            louvain_with_resolution_sync(local_edges.as_slice(), config.community_resolution);
        let mut local_to_global = BTreeMap::<usize, usize>::new();
        for (_, local_id) in &local_assignments {
            local_to_global.entry(*local_id).or_insert_with(|| {
                let assigned = next_community_id;
                next_community_id += 1;
                assigned
            });
        }

        for (rep_name, local_id) in local_assignments {
            let Some(rep) = rep_name
                .strip_prefix("rep-")
                .and_then(|value| value.parse::<usize>().ok())
            else {
                continue;
            };
            if let Some(global_id) = local_to_global.get(&local_id).copied() {
                rep_to_community.insert(rep, global_id);
            }
        }
    }
    let communities_before_merge = rep_to_community
        .values()
        .copied()
        .collect::<HashSet<_>>()
        .len();

    let (rep_to_community, unmerged_small_penalty) = merge_small_communities(
        rep_to_community,
        entries.as_slice(),
        rep_to_members.as_slice(),
        &component_of_rep,
        &structural_graph,
        config.min_community_size,
    );
    let communities_after_merge = rep_to_community
        .values()
        .copied()
        .collect::<HashSet<_>>()
        .len();

    let assignments = finalize_assignments(
        entries.as_slice(),
        rep_by_index.as_slice(),
        &loner_reps,
        &rep_to_community,
    );

    DetectionRun {
        assignments,
        symbols_total,
        symbols_filtered_test,
        symbols_anchored_type,
        symbols_rescued_container,
        symbols_rescued_semantic,
        symbols_loner,
        communities_before_merge,
        communities_after_merge,
        embedding_coverage_pct,
        non_test_count,
        unmerged_small_penalty,
    }
}

const ANCHOR_SPLIT_THRESHOLD: usize = 20;
const ANCHOR_MIN_BUCKET: usize = 3;
const ANCHOR_STOPWORDS: &[&str] = &[
    "get",
    "set",
    "list",
    "find",
    "read",
    "write",
    "upsert",
    "delete",
    "remove",
    "insert",
    "update",
    "create",
    "mark",
    "clear",
    "prune",
    "count",
    "increment",
    "record",
    "load",
    "save",
    "search",
    "open",
    "close",
    "new",
    "default",
    "from",
    "into",
    "with",
    "for",
    "the",
    "and",
    "is",
    "has",
    "all",
    "batch",
    "by",
    "if",
    "or",
    "run",
    "do",
    "try",
    "check",
    "ensure",
    "acknowledge",
    "resolve",
    "as",
    "to",
    "sync",
    "test",
];

fn normalize_anchor_token(token: &str) -> String {
    let normalized = token
        .strip_prefix("r#")
        .unwrap_or(token)
        .to_ascii_lowercase();
    match normalized.as_str() {
        "note" | "notes" => "note".to_owned(),
        "project" | "projects" => "project".to_owned(),
        "migration" | "migrate" | "migrations" => "migration".to_owned(),
        "embedding" | "embeddings" => "embedding".to_owned(),
        "symbol" | "symbols" => "symbol".to_owned(),
        "intent" | "intents" => "intent".to_owned(),
        "store" | "stores" => "store".to_owned(),
        "edge" | "edges" => "edge".to_owned(),
        "version" | "versions" => "version".to_owned(),
        "request" | "requests" => "request".to_owned(),
        "result" | "results" => "result".to_owned(),
        "graph" | "graphs" => "graph".to_owned(),
        "schema" | "schemas" => "schema".to_owned(),
        "module" | "modules" => "module".to_owned(),
        "provider" | "providers" => "provider".to_owned(),
        "model" | "models" => "model".to_owned(),
        "meta" | "metas" => "meta".to_owned(),
        "history" | "histories" => "history".to_owned(),
        other => {
            if other.len() > 3 {
                other.strip_suffix('s').unwrap_or(other).to_owned()
            } else {
                other.to_owned()
            }
        }
    }
}

fn informative_tokens(name: &str) -> Vec<String> {
    let leaf_name = name.rsplit("::").next().unwrap_or(name);
    leaf_name
        .split('_')
        .filter_map(|token| {
            let normalized = normalize_anchor_token(token);
            if normalized.len() <= 1 || ANCHOR_STOPWORDS.contains(&normalized.as_str()) {
                None
            } else {
                Some(normalized)
            }
        })
        .collect()
}

fn split_large_anchor_groups(
    entries: &[SymbolEntry],
    original_groups: Vec<Vec<usize>>,
) -> (Vec<Vec<usize>>, HashSet<usize>) {
    let mut split_groups = Vec::new();
    let mut split_members = HashSet::new();

    for mut group in original_groups {
        if group.is_empty() {
            continue;
        }
        group.sort_unstable();

        if group.len() <= ANCHOR_SPLIT_THRESHOLD {
            split_groups.push(group);
            continue;
        }
        if !group
            .iter()
            .any(|index| is_type_anchor(entries[*index].symbol.kind))
        {
            split_groups.push(group);
            continue;
        }

        let mut anchor_indices = Vec::new();
        let mut method_indices = Vec::new();
        for index in &group {
            if is_type_anchor(entries[*index].symbol.kind) {
                anchor_indices.push(*index);
            } else {
                method_indices.push(*index);
            }
        }
        if method_indices.len() < ANCHOR_SPLIT_THRESHOLD {
            split_groups.push(group);
            continue;
        }

        let mut method_tokens = HashMap::<usize, Vec<String>>::new();
        for index in &method_indices {
            let name = entries[*index].symbol.qualified_name.as_str();
            let tokens = informative_tokens(name);
            method_tokens.insert(*index, tokens);
        }

        let mut bucket_members = BTreeMap::<String, Vec<usize>>::new();
        for index in &method_indices {
            let bucket_key = method_tokens
                .get(index)
                .and_then(|tokens| tokens.first())
                .cloned()
                .unwrap_or_else(|| "misc".to_owned());
            bucket_members.entry(bucket_key).or_default().push(*index);
        }

        let has_large_bucket = bucket_members
            .values()
            .any(|members| members.len() >= ANCHOR_MIN_BUCKET);
        if !has_large_bucket {
            split_groups.push(group);
            continue;
        }

        let mut bucket_tokens = bucket_members
            .iter()
            .map(|(key, members)| {
                let tokens = members
                    .iter()
                    .filter_map(|member| method_tokens.get(member))
                    .flat_map(|tokens| tokens.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                (key.clone(), tokens)
            })
            .collect::<HashMap<_, _>>();
        let small_bucket_keys = bucket_members
            .iter()
            .filter_map(|(key, members)| (members.len() < ANCHOR_MIN_BUCKET).then_some(key.clone()))
            .collect::<Vec<_>>();
        for small_key in small_bucket_keys {
            let Some(small_members) = bucket_members.remove(&small_key) else {
                continue;
            };
            let small_tokens = bucket_tokens.remove(&small_key).unwrap_or_default();
            let target_key = bucket_members
                .iter()
                .filter(|(_, members)| members.len() >= ANCHOR_MIN_BUCKET)
                .map(|(key, members)| {
                    let target_tokens = bucket_tokens
                        .get(key)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let target_tokens = target_tokens.iter().collect::<HashSet<_>>();
                    (
                        key.clone(),
                        small_tokens
                            .iter()
                            .filter(|token| target_tokens.contains(token))
                            .count(),
                        members.len(),
                    )
                })
                .max_by(|left, right| {
                    left.1
                        .cmp(&right.1)
                        .then_with(|| left.2.cmp(&right.2))
                        .then_with(|| right.0.cmp(&left.0))
                })
                .map(|(key, _, _)| key);

            let Some(target_key) = target_key else {
                bucket_members.insert(small_key, small_members);
                continue;
            };

            if let Some(target_members) = bucket_members.get_mut(&target_key) {
                target_members.extend(small_members);
            }
            if let Some(target_tokens) = bucket_tokens.get_mut(&target_key) {
                let mut merged_tokens = target_tokens.iter().cloned().collect::<BTreeSet<_>>();
                merged_tokens.extend(small_tokens);
                *target_tokens = merged_tokens.into_iter().collect();
            }
        }

        if bucket_members.len() <= 1 {
            split_groups.push(group);
            continue;
        }

        split_members.extend(group.iter().copied());

        if !anchor_indices.is_empty()
            && let Some(target_key) = bucket_members
                .iter()
                .max_by(|left, right| {
                    left.1
                        .len()
                        .cmp(&right.1.len())
                        .then_with(|| right.0.cmp(left.0))
                })
                .map(|(key, _)| key.clone())
            && let Some(target_members) = bucket_members.get_mut(&target_key)
        {
            target_members.extend(anchor_indices);
        }

        for (_, mut members) in bucket_members {
            members.sort_unstable();
            split_groups.push(members);
        }
    }

    (split_groups, split_members)
}

fn count_type_anchored_symbols(entries: &[SymbolEntry], groups: &[Vec<usize>]) -> usize {
    groups
        .iter()
        .filter(|members| {
            members.len() > 1
                && members
                    .iter()
                    .any(|index| is_type_anchor(entries[*index].symbol.kind))
        })
        .map(Vec::len)
        .sum()
}

fn build_anchor_groups(entries: &[SymbolEntry]) -> (DisjointSet, Vec<Vec<usize>>) {
    let mut union_find = DisjointSet::new(entries.len());
    let mut stem_to_indices = HashMap::<String, Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        stem_to_indices
            .entry(entry.stem.clone())
            .or_default()
            .push(index);
    }

    for (index, entry) in entries.iter().enumerate() {
        if !is_type_anchor(entry.symbol.kind) {
            continue;
        }
        let mut members = vec![index];
        if let Some(group) = stem_to_indices.get(entry.symbol.qualified_name.as_str()) {
            for other in group {
                if *other != index {
                    members.push(*other);
                }
            }
        }
        if members.len() < 2 {
            continue;
        }
        let anchor = members[0];
        for member in members.into_iter().skip(1) {
            union_find.union(anchor, member);
        }
    }

    let rep_to_members = build_rep_members(entries.len(), &mut union_find);
    (union_find, rep_to_members)
}

fn rebuild_union_find_from_groups(num_entries: usize, groups: &[Vec<usize>]) -> DisjointSet {
    let mut union_find = DisjointSet::new(num_entries);
    for group in groups {
        let Some(anchor) = group.first().copied() else {
            continue;
        };
        for member in group.iter().copied().skip(1) {
            union_find.union(anchor, member);
        }
    }
    union_find
}

fn collapse_structural_edges(
    edges: &[GraphAlgorithmEdge],
    id_to_index: &HashMap<String, usize>,
    union_find: &mut DisjointSet,
) -> WeightedGraph {
    let mut graph = WeightedGraph::default();
    for edge in edges {
        let Some(source) = id_to_index.get(edge.source_id.as_str()).copied() else {
            continue;
        };
        let Some(target) = id_to_index.get(edge.target_id.as_str()).copied() else {
            continue;
        };
        let source_rep = union_find.find(source);
        let target_rep = union_find.find(target);
        graph.add_edge(source_rep, target_rep, 1);
    }
    graph
}

#[allow(dead_code)]
fn apply_container_rescue(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    rep_to_members: &[Vec<usize>],
    graph: &mut WeightedGraph,
) -> usize {
    let split_exclusions = HashSet::new();
    apply_container_rescue_with_exclusions(
        entries,
        rep_by_index,
        rep_to_members,
        graph,
        &split_exclusions,
    )
}

fn apply_container_rescue_with_exclusions(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    rep_to_members: &[Vec<usize>],
    graph: &mut WeightedGraph,
    split_exclusions: &HashSet<usize>,
) -> usize {
    let mut stem_to_singleton_reps = HashMap::<String, Vec<usize>>::new();
    for (index, entry) in entries.iter().enumerate() {
        let rep = rep_by_index.get(index).copied().unwrap_or(index);
        if rep_to_members.get(rep).map(Vec::len).unwrap_or_default() != 1 {
            continue;
        }
        if split_exclusions.contains(&index) {
            continue;
        }
        if entry.stem.is_empty() {
            continue;
        }
        stem_to_singleton_reps
            .entry(entry.stem.clone())
            .or_default()
            .push(rep);
    }

    let mut rescued = HashSet::new();
    let mut added_pairs = HashSet::new();
    for reps in stem_to_singleton_reps.values() {
        let mut unique_reps = reps
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if unique_reps.len() < 2 {
            continue;
        }
        unique_reps.sort_unstable();

        let zero_degree_sources = unique_reps
            .iter()
            .copied()
            .filter(|rep| graph.degree(*rep) == 0)
            .collect::<Vec<_>>();
        for source in zero_degree_sources {
            let mut rescued_source = false;
            for target in &unique_reps {
                if *target == source {
                    continue;
                }
                let pair = normalized_pair(source, *target);
                if added_pairs.insert(pair) {
                    graph.add_edge(source, *target, 1);
                }
                rescued_source = true;
            }
            if rescued_source
                && let Some(member) = rep_to_members
                    .get(source)
                    .and_then(|members| members.first())
                    .copied()
            {
                rescued.insert(member);
            }
        }
    }
    rescued.len()
}

fn apply_semantic_rescue(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    graph: &mut WeightedGraph,
    config: &FileCommunityConfig,
) -> usize {
    let source_indices = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if !has_embedding(entry.symbol.embedding.as_deref()) {
                return None;
            }
            let rep = rep_by_index.get(index).copied().unwrap_or(index);
            if graph.degree(rep) <= 1 {
                Some(index)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    let mut rescued = HashSet::new();
    for source_index in source_indices {
        let Some(source_embedding) = entries
            .get(source_index)
            .and_then(|entry| entry.symbol.embedding.as_deref())
        else {
            continue;
        };
        let source_rep = rep_by_index
            .get(source_index)
            .copied()
            .unwrap_or(source_index);
        let mut candidates = entries
            .iter()
            .enumerate()
            .filter_map(|(target_index, target)| {
                if source_index == target_index {
                    return None;
                }
                let target_rep = rep_by_index
                    .get(target_index)
                    .copied()
                    .unwrap_or(target_index);
                if target_rep == source_rep || graph.degree(target_rep) >= 5 {
                    return None;
                }
                let similarity =
                    cosine_similarity(Some(source_embedding), target.symbol.embedding.as_deref())?;
                Some((similarity, target.symbol.symbol_id.as_str(), target_rep))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(right.1))
        });

        let Some((best_similarity, _, _)) = candidates.first().copied() else {
            continue;
        };
        if best_similarity < config.semantic_rescue_threshold {
            continue;
        }

        let mut added = false;
        for (_, _, target_rep) in candidates.into_iter().take(config.semantic_rescue_max_k) {
            graph.add_edge(source_rep, target_rep, 1);
            added = true;
        }
        if added {
            rescued.insert(source_index);
        }
    }

    rescued.len()
}

fn merge_small_communities(
    mut rep_to_community: HashMap<usize, usize>,
    entries: &[SymbolEntry],
    rep_to_members: &[Vec<usize>],
    component_of_rep: &HashMap<usize, usize>,
    structural_graph: &WeightedGraph,
    min_community_size: usize,
) -> (HashMap<usize, usize>, f32) {
    let mut penalty = 0.0_f32;
    let mut penalized = HashSet::new();

    loop {
        let communities = build_communities(&rep_to_community, rep_to_members, component_of_rep);
        let mut small_ids = communities
            .iter()
            .filter_map(|(community_id, state)| {
                if state.symbol_count < min_community_size {
                    Some((*community_id, state.symbol_count))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        small_ids.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        if small_ids.is_empty() {
            break;
        }

        let mut changed = false;
        for (community_id, _) in small_ids {
            let current = build_communities(&rep_to_community, rep_to_members, component_of_rep);
            let Some(small) = current.get(&community_id) else {
                continue;
            };
            if small.symbol_count >= min_community_size {
                continue;
            }

            let candidates = current
                .iter()
                .filter_map(|(target_id, target)| {
                    if *target_id == community_id
                        || target.component_id != small.component_id
                        || target.symbol_count <= small.symbol_count
                    {
                        return None;
                    }
                    Some(MergeCandidate {
                        community_id: *target_id,
                        structural_edges: structural_edges_between(
                            structural_graph,
                            &rep_to_community,
                            community_id,
                            *target_id,
                        ),
                        semantic_affinity: semantic_affinity(
                            entries,
                            small.symbol_indices.as_slice(),
                            target.symbol_indices.as_slice(),
                        ),
                        size: target.symbol_count,
                    })
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }

            let has_structural_signal = candidates
                .iter()
                .any(|candidate| candidate.structural_edges > 0);
            let best = if has_structural_signal {
                candidates.into_iter().max_by(compare_merge_candidates)
            } else {
                let semantic_candidates = candidates
                    .into_iter()
                    .filter(|candidate| candidate.semantic_affinity.is_some())
                    .collect::<Vec<_>>();
                if semantic_candidates.is_empty() {
                    None
                } else {
                    semantic_candidates
                        .into_iter()
                        .max_by(compare_merge_candidates)
                }
            };

            let Some(best) = best else {
                if penalized.insert(community_id) {
                    penalty += 0.1;
                }
                continue;
            };

            for rep in &small.reps {
                rep_to_community.insert(*rep, best.community_id);
            }
            changed = true;
        }

        if !changed {
            break;
        }
    }

    (rep_to_community, penalty)
}

fn finalize_assignments(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    loner_reps: &HashSet<usize>,
    rep_to_community: &HashMap<usize, usize>,
) -> Vec<(String, usize)> {
    let mut community_ids = rep_to_community
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    community_ids.sort_unstable();
    let dense_ids = community_ids
        .into_iter()
        .enumerate()
        .map(|(index, community_id)| (community_id, index + 1))
        .collect::<HashMap<_, _>>();

    let mut assignments = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let rep = rep_by_index.get(index).copied().unwrap_or(index);
            if loner_reps.contains(&rep) {
                return None;
            }
            let community = rep_to_community.get(&rep).copied()?;
            let dense_id = dense_ids.get(&community).copied()?;
            Some((entry.symbol.symbol_id.clone(), dense_id))
        })
        .collect::<Vec<_>>();
    assignments.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    assignments
}

fn build_communities(
    rep_to_community: &HashMap<usize, usize>,
    rep_to_members: &[Vec<usize>],
    component_of_rep: &HashMap<usize, usize>,
) -> BTreeMap<usize, CommunityState> {
    let mut communities = BTreeMap::<usize, CommunityState>::new();
    for (rep, community_id) in rep_to_community {
        let members = rep_to_members.get(*rep).cloned().unwrap_or_default();
        let component_id = component_of_rep.get(rep).copied().unwrap_or_default();
        let state = communities
            .entry(*community_id)
            .or_insert_with(|| CommunityState {
                reps: Vec::new(),
                symbol_indices: Vec::new(),
                symbol_count: 0,
                component_id,
            });
        state.reps.push(*rep);
        state.symbol_count += members.len();
        state.symbol_indices.extend(members);
    }
    for state in communities.values_mut() {
        state.reps.sort_unstable();
        state.symbol_indices.sort_unstable();
        state.symbol_indices.dedup();
    }
    communities
}

fn structural_edges_between(
    graph: &WeightedGraph,
    rep_to_community: &HashMap<usize, usize>,
    left_community: usize,
    right_community: usize,
) -> usize {
    graph
        .edges
        .iter()
        .filter_map(|((left_rep, right_rep), weight)| {
            let left = rep_to_community.get(left_rep).copied()?;
            let right = rep_to_community.get(right_rep).copied()?;
            if (left == left_community && right == right_community)
                || (left == right_community && right == left_community)
            {
                Some(*weight)
            } else {
                None
            }
        })
        .sum()
}

fn semantic_affinity(
    entries: &[SymbolEntry],
    left_indices: &[usize],
    right_indices: &[usize],
) -> Option<f32> {
    let mut total = 0.0_f32;
    let mut pairs = 0usize;
    for left in left_indices {
        let Some(left_embedding) = entries
            .get(*left)
            .and_then(|entry| entry.symbol.embedding.as_deref())
        else {
            continue;
        };
        for right in right_indices {
            let Some(right_embedding) = entries
                .get(*right)
                .and_then(|entry| entry.symbol.embedding.as_deref())
            else {
                continue;
            };
            let Some(similarity) = cosine_similarity(Some(left_embedding), Some(right_embedding))
            else {
                continue;
            };
            total += similarity;
            pairs += 1;
        }
    }

    if pairs == 0 {
        None
    } else {
        Some(total / pairs as f32)
    }
}

fn compare_merge_candidates(left: &MergeCandidate, right: &MergeCandidate) -> Ordering {
    left.structural_edges
        .cmp(&right.structural_edges)
        .then_with(|| compare_optional_f32(left.semantic_affinity, right.semantic_affinity))
        .then_with(|| left.size.cmp(&right.size))
        .then_with(|| right.community_id.cmp(&left.community_id))
}

fn diagnostics_from_run(
    run: &DetectionRun,
    stability_score: f32,
    confidence: f32,
) -> PlannerDiagnostics {
    PlannerDiagnostics {
        symbols_total: run.symbols_total,
        symbols_filtered_test: run.symbols_filtered_test,
        symbols_anchored_type: run.symbols_anchored_type,
        symbols_rescued_container: run.symbols_rescued_container,
        symbols_rescued_semantic: run.symbols_rescued_semantic,
        symbols_loner: run.symbols_loner,
        communities_before_merge: run.communities_before_merge,
        communities_after_merge: run.communities_after_merge,
        embedding_coverage_pct: run.embedding_coverage_pct,
        confidence,
        confidence_label: confidence_label(confidence).to_owned(),
        stability_score,
    }
}

fn compute_confidence(run: &DetectionRun, stability_score: f32) -> f32 {
    if run.non_test_count == 0 {
        return 0.0;
    }

    let non_test_count = run.non_test_count as f32;
    let rescue_ratio =
        (run.symbols_rescued_container + run.symbols_rescued_semantic) as f32 / non_test_count;
    let loner_ratio = run.symbols_loner as f32 / non_test_count;
    let mut confidence = 1.0
        - 0.3 * rescue_ratio
        - 0.2 * loner_ratio
        - 0.2 * (1.0 - run.embedding_coverage_pct)
        - 0.3 * (1.0 - stability_score);
    confidence -= run.unmerged_small_penalty;
    confidence.clamp(0.0, 1.0)
}

fn confidence_label(confidence: f32) -> &'static str {
    if confidence >= 0.7 {
        "high"
    } else if confidence >= 0.4 {
        "medium"
    } else {
        "low"
    }
}

fn pairwise_jaccard(baseline: &[(String, usize)], perturbed: &[(String, usize)]) -> f32 {
    let left = same_community_pairs(baseline);
    let right = same_community_pairs(perturbed);
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let intersection = left.intersection(&right).count();
    let union = left.union(&right).count();
    if union == 0 {
        1.0
    } else {
        intersection as f32 / union as f32
    }
}

fn same_community_pairs(assignments: &[(String, usize)]) -> HashSet<(String, String)> {
    let mut by_community = BTreeMap::<usize, Vec<&str>>::new();
    for (symbol_id, community_id) in assignments {
        by_community
            .entry(*community_id)
            .or_default()
            .push(symbol_id.as_str());
    }

    let mut pairs = HashSet::new();
    for symbols in by_community.values_mut() {
        symbols.sort_unstable();
        for (index, left) in symbols.iter().enumerate() {
            for right in symbols.iter().skip(index + 1) {
                pairs.insert(((*left).to_owned(), (*right).to_owned()));
            }
        }
    }
    pairs
}

fn build_rep_members(len: usize, union_find: &mut DisjointSet) -> Vec<Vec<usize>> {
    let mut reps = vec![Vec::new(); len];
    for index in 0..len {
        let rep = union_find.find(index);
        if let Some(slot) = reps.get_mut(rep) {
            slot.push(index);
        }
    }
    reps
}

fn min_symbol_id_for_rep(rep: usize, entries: &[SymbolEntry]) -> String {
    entries
        .get(rep)
        .map(|entry| entry.symbol.symbol_id.clone())
        .unwrap_or_default()
}

fn min_symbol_id_for_component(component: &[usize], entries: &[SymbolEntry]) -> String {
    component
        .iter()
        .filter_map(|rep| {
            entries
                .get(*rep)
                .map(|entry| entry.symbol.symbol_id.clone())
        })
        .min()
        .unwrap_or_default()
}

fn normalized_pair(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn compare_optional_f32(left: Option<f32>, right: Option<f32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

fn qualified_name_stem(value: &str) -> String {
    value
        .rsplit_once("::")
        .map(|(stem, _)| stem.to_owned())
        .unwrap_or_default()
}

fn is_type_anchor(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait | SymbolKind::TypeAlias
    )
}

fn has_embedding(embedding: Option<&[f32]>) -> bool {
    embedding.is_some_and(|values| {
        !values.is_empty()
            && values.iter().all(|value| value.is_finite())
            && values.iter().map(|value| value * value).sum::<f32>() > f32::EPSILON
    })
}

fn cosine_similarity(left: Option<&[f32]>, right: Option<&[f32]>) -> Option<f32> {
    let left = left?;
    let right = right?;
    if left.len() != right.len() || left.is_empty() {
        return None;
    }

    let mut dot = 0.0_f32;
    let mut left_norm_sq = 0.0_f32;
    let mut right_norm_sq = 0.0_f32;
    for (left_value, right_value) in left.iter().zip(right.iter()) {
        if !left_value.is_finite() || !right_value.is_finite() {
            return None;
        }
        dot += left_value * right_value;
        left_norm_sq += left_value * left_value;
        right_norm_sq += right_value * right_value;
    }
    if left_norm_sq <= f32::EPSILON || right_norm_sq <= f32::EPSILON {
        return None;
    }

    Some(dot / (left_norm_sq.sqrt() * right_norm_sq.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::{
        DetectionRun, DisjointSet, FileCommunityConfig, FileSymbol, PlannerDiagnostics,
        SymbolEntry, WeightedGraph, apply_container_rescue, apply_container_rescue_with_exclusions,
        apply_semantic_rescue, build_anchor_groups, build_rep_members, collapse_structural_edges,
        compute_confidence, confidence_label, count_type_anchored_symbols, detect_file_communities,
        diagnostics_from_run, finalize_assignments, has_embedding, merge_small_communities,
        pairwise_jaccard, qualified_name_stem, rebuild_union_find_from_groups,
        split_large_anchor_groups,
    };
    use aether_config::load_workspace_config;
    use aether_core::SymbolKind;
    use aether_graph_algo::GraphAlgorithmEdge;
    use aether_store::{SqliteStore, Store, SurrealGraphStore, open_vector_store};
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

    const ABLATION_STOPWORDS: &[&str] = &[
        "default", "new", "from", "into", "load", "save", "get", "set", "is", "has", "with", "for",
        "the", "and", "fn", "test", "mock", "impl", "try", "run", "do",
    ];

    #[derive(Clone, Copy)]
    struct AblationOptions {
        filter_tests: bool,
        type_anchor: bool,
        container_rescue: bool,
        semantic_rescue: bool,
        community_resolution: f64,
        merge_small: bool,
    }

    #[derive(Clone)]
    struct AblationInput {
        crate_name: String,
        file_path: String,
        symbols: Vec<FileSymbol>,
        edges: Vec<GraphAlgorithmEdge>,
        config: FileCommunityConfig,
    }

    fn config() -> FileCommunityConfig {
        FileCommunityConfig {
            semantic_rescue_threshold: 0.70,
            semantic_rescue_max_k: 3,
            community_resolution: 0.5,
            min_community_size: 3,
        }
    }

    fn symbol(symbol_id: &str, name: &str, qualified_name: &str, kind: SymbolKind) -> FileSymbol {
        FileSymbol {
            symbol_id: symbol_id.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            kind,
            is_test: false,
            embedding: None,
        }
    }

    fn edge(source_id: &str, target_id: &str) -> GraphAlgorithmEdge {
        GraphAlgorithmEdge {
            source_id: source_id.to_owned(),
            target_id: target_id.to_owned(),
            edge_kind: "calls".to_owned(),
        }
    }

    fn with_embedding(mut symbol: FileSymbol, embedding: &[f32]) -> FileSymbol {
        symbol.embedding = Some(embedding.to_vec());
        symbol
    }

    fn entries(symbols: &[FileSymbol]) -> Vec<SymbolEntry> {
        symbols
            .iter()
            .cloned()
            .map(|symbol| SymbolEntry {
                stem: qualified_name_stem(symbol.qualified_name.as_str()),
                symbol,
            })
            .collect()
    }

    fn assignment_map(assignments: &[(String, usize)]) -> HashMap<&str, usize> {
        assignments
            .iter()
            .map(|(symbol_id, community_id)| (symbol_id.as_str(), *community_id))
            .collect()
    }

    fn community_ids(assignments: &[(String, usize)]) -> HashSet<usize> {
        assignments
            .iter()
            .map(|(_, community_id)| *community_id)
            .collect()
    }

    fn rep_members(entries: &[SymbolEntry]) -> (Vec<usize>, Vec<Vec<usize>>) {
        let (mut union_find, _) = build_anchor_groups(entries);
        let rep_to_members = build_rep_members(entries.len(), &mut union_find);
        let rep_by_index = (0..entries.len())
            .map(|index| union_find.find(index))
            .collect::<Vec<_>>();
        (rep_by_index, rep_to_members)
    }

    fn count_components_and_largest(
        graph: &WeightedGraph,
        rep_to_members: &[Vec<usize>],
        entries: &[SymbolEntry],
    ) -> (usize, usize) {
        let active_reps: Vec<usize> = rep_to_members
            .iter()
            .enumerate()
            .filter_map(
                |(rep, members)| {
                    if members.is_empty() { None } else { Some(rep) }
                },
            )
            .collect();
        if active_reps.is_empty() {
            return (0, 0);
        }
        let components = graph.connected_components(&active_reps, entries);
        let largest = components
            .iter()
            .map(|component| {
                component
                    .iter()
                    .map(|rep| rep_to_members.get(*rep).map(Vec::len).unwrap_or(0))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        (components.len(), largest)
    }

    fn non_empty_groups(groups: &[Vec<usize>]) -> Vec<Vec<usize>> {
        groups
            .iter()
            .filter(|members| !members.is_empty())
            .cloned()
            .collect()
    }

    fn split_anchor_groups(entries: &[SymbolEntry]) -> (Vec<Vec<usize>>, HashSet<usize>) {
        let (_, anchor_groups) = build_anchor_groups(entries);
        split_large_anchor_groups(entries, anchor_groups)
    }

    fn group_index_for_symbol_id(
        groups: &[Vec<usize>],
        entries: &[SymbolEntry],
        symbol_id: &str,
    ) -> Option<usize> {
        groups.iter().position(|members| {
            members.iter().any(|index| {
                entries
                    .get(*index)
                    .map(|entry| entry.symbol.symbol_id.as_str() == symbol_id)
                    .unwrap_or(false)
            })
        })
    }

    fn large_anchor_symbols() -> Vec<FileSymbol> {
        let mut symbols = vec![symbol(
            "sym-store",
            "SqliteStore",
            "crate::SqliteStore",
            SymbolKind::Struct,
        )];
        let methods = [
            ("sym-project-note-upsert", "upsert_project_note"),
            ("sym-project-note-list", "list_project_notes"),
            ("sym-project-note-delete", "delete_project_note"),
            ("sym-project-note-find", "find_project_note"),
            ("sym-project-note-read", "read_project_notes"),
            ("sym-project-note-write", "write_project_note"),
            ("sym-project-note-save", "save_project_note"),
            ("sym-project-note-load", "load_project_notes"),
            ("sym-project-note-count", "count_project_notes"),
            ("sym-sir-get", "get_sir"),
            ("sym-sir-save", "save_sir"),
            ("sym-sir-load", "load_sir"),
            ("sym-sir-list", "list_sirs"),
            ("sym-sir-meta", "upsert_sir_meta"),
            ("sym-sir-history", "list_sir_history"),
            ("sym-sir-version", "get_sir_version"),
            ("sym-sir-schema", "read_sir_schema"),
            ("sym-migration-run", "run_migrations"),
            ("sym-migration-list", "list_migrations"),
            ("sym-migration-delete", "delete_migrations"),
            ("sym-migration-clear", "clear_migrations"),
            ("sym-migration-rename", "migration_v6_renames"),
            ("sym-migration-from", "migration_from_v2"),
            ("sym-intent-list", "list_intents"),
            ("sym-intent-find", "find_intents"),
            ("sym-intent-delete", "delete_intents"),
            ("sym-intent-write", "create_write_intent"),
            ("sym-intent-status", "update_intent_status"),
            ("sym-intent-failed", "mark_intent_failed"),
            ("sym-intent-count", "count_intents"),
        ];
        for (symbol_id, method_name) in methods {
            symbols.push(symbol(
                symbol_id,
                method_name,
                &format!("crate::SqliteStore::{method_name}"),
                SymbolKind::Method,
            ));
        }
        symbols
    }

    #[test]
    fn all_loners_returns_empty_assignments() {
        let symbols = vec![
            symbol(
                "sym-a",
                "alpha",
                "crate::alpha::handler",
                SymbolKind::Function,
            ),
            symbol(
                "sym-b",
                "beta",
                "crate::beta::handler",
                SymbolKind::Function,
            ),
        ];

        let (assignments, diagnostics) = detect_file_communities(&[], &symbols, &config());
        assert!(assignments.is_empty());
        assert_eq!(diagnostics.symbols_loner, 2);
        assert_eq!(diagnostics.confidence_label, "low");
    }

    #[test]
    fn confidence_reflects_diagnostics() {
        let mut symbols = vec![
            symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
            symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
            symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
        ];
        symbols[0].embedding = Some(vec![1.0, 0.0]);
        symbols[1].embedding = Some(vec![0.9, 0.1]);

        let (assignments, diagnostics) =
            detect_file_communities(&[edge("sym-a", "sym-b")], &symbols, &config());

        assert!(!assignments.is_empty());
        assert!(diagnostics.confidence >= 0.0);
        assert_eq!(
            diagnostics.confidence_label,
            confidence_label(diagnostics.confidence)
        );
    }

    #[test]
    fn diagnostics_reports_accurate_counts() {
        let mut symbols = vec![
            symbol("sym-type", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-method",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol(
                "sym-test",
                "test_widget",
                "crate::test_widget",
                SymbolKind::Function,
            ),
        ];
        symbols[2].is_test = true;

        let (_, diagnostics) = detect_file_communities(&[], &symbols, &config());
        assert_eq!(diagnostics.symbols_total, 3);
        assert_eq!(diagnostics.symbols_filtered_test, 1);
        assert_eq!(diagnostics.symbols_anchored_type, 2);
    }

    #[test]
    fn stability_check_returns_high_for_stable_graph() {
        let symbols = vec![
            symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
            symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
            symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
            symbol("sym-d", "delta", "crate::delta", SymbolKind::Function),
        ];
        let edges = vec![edge("sym-a", "sym-b"), edge("sym-c", "sym-d")];

        let (_, diagnostics) = detect_file_communities(&edges, &symbols, &config());
        assert!(diagnostics.stability_score >= 0.9);
    }

    #[test]
    fn filter_tests_before_graph_construction() {
        let mut test_symbol = symbol(
            "sym-test",
            "test_alpha",
            "crate::test_alpha",
            SymbolKind::Function,
        );
        test_symbol.is_test = true;
        let symbols = vec![
            symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
            symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
            test_symbol,
        ];
        let edges = vec![
            edge("sym-a", "sym-b"),
            edge("sym-test", "sym-a"),
            edge("sym-test", "sym-b"),
        ];

        let (assignments, diagnostics) = detect_file_communities(&edges, &symbols, &config());
        let assignment_map = assignment_map(&assignments);
        assert_eq!(assignment_map.len(), 2);
        assert!(assignment_map.contains_key("sym-a"));
        assert!(assignment_map.contains_key("sym-b"));
        assert!(!assignment_map.contains_key("sym-test"));
        assert_eq!(diagnostics.symbols_filtered_test, 1);
    }

    #[test]
    fn type_anchor_precollapse_keeps_type_and_methods_together() {
        let symbols = vec![
            symbol("sym-type", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-method",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol(
                "sym-helper",
                "helper",
                "crate::helper",
                SymbolKind::Function,
            ),
        ];

        let (mut union_find, _) = build_anchor_groups(entries(&symbols).as_slice());
        let type_rep = union_find.find(0);
        let method_rep = union_find.find(1);
        let helper_rep = union_find.find(2);

        assert_eq!(type_rep, method_rep);
        assert_ne!(type_rep, helper_rep);
    }

    #[test]
    fn type_anchor_does_not_cross_types() {
        let symbols = vec![
            symbol("sym-widget", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-widget-method",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol("sym-gadget", "Gadget", "crate::Gadget", SymbolKind::Struct),
            symbol(
                "sym-gadget-method",
                "run",
                "crate::Gadget::run",
                SymbolKind::Method,
            ),
        ];

        let (mut union_find, _) = build_anchor_groups(entries(&symbols).as_slice());
        let widget_rep = union_find.find(0);
        let widget_method_rep = union_find.find(1);
        let gadget_rep = union_find.find(2);
        let gadget_method_rep = union_find.find(3);

        assert_eq!(widget_rep, widget_method_rep);
        assert_eq!(gadget_rep, gadget_method_rep);
        assert_ne!(widget_rep, gadget_rep);
    }

    #[test]
    fn split_large_anchor_skips_small_groups() {
        let mut symbols = vec![symbol(
            "sym-store",
            "SqliteStore",
            "crate::SqliteStore",
            SymbolKind::Struct,
        )];
        for index in 0..9 {
            symbols.push(symbol(
                &format!("sym-method-{index}"),
                &format!("op_{index}"),
                &format!("crate::SqliteStore::op_{index}"),
                SymbolKind::Method,
            ));
        }

        let entries = entries(&symbols);
        let (_, anchor_groups) = build_anchor_groups(entries.as_slice());
        let expected_groups = non_empty_groups(anchor_groups.as_slice());
        let (split_groups, split_members) =
            split_large_anchor_groups(entries.as_slice(), anchor_groups);

        assert_eq!(split_groups, expected_groups);
        assert!(split_members.is_empty());
    }

    #[test]
    fn split_large_anchor_partitions_by_domain_token() {
        let entries = entries(&large_anchor_symbols());
        let (split_groups, split_members) = split_anchor_groups(entries.as_slice());

        assert!(split_groups.len() > 1);
        assert_eq!(split_members.len(), entries.len());

        let sir_group =
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-sir-meta");
        assert_eq!(
            sir_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-sir-history"
            )
        );
        assert_eq!(
            sir_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-sir-version"
            )
        );

        let project_group = group_index_for_symbol_id(
            split_groups.as_slice(),
            entries.as_slice(),
            "sym-project-note-upsert",
        );
        assert_eq!(
            project_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-project-note-list",
            )
        );
        assert_eq!(
            project_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-project-note-delete",
            )
        );

        let migration_group = group_index_for_symbol_id(
            split_groups.as_slice(),
            entries.as_slice(),
            "sym-migration-run",
        );
        assert_eq!(
            migration_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-migration-rename",
            )
        );
        assert_eq!(
            migration_group,
            group_index_for_symbol_id(
                split_groups.as_slice(),
                entries.as_slice(),
                "sym-migration-from",
            )
        );

        assert_ne!(sir_group, project_group);
        assert_ne!(sir_group, migration_group);
        assert_eq!(
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-store"),
            project_group
        );
    }

    #[test]
    fn split_large_anchor_type_not_singleton() {
        let entries = entries(&large_anchor_symbols());
        let (split_groups, _) = split_anchor_groups(entries.as_slice());
        let store_group =
            group_index_for_symbol_id(split_groups.as_slice(), entries.as_slice(), "sym-store");

        assert!(store_group.is_some());
        if let Some(group_index) = store_group {
            assert!(
                split_groups
                    .get(group_index)
                    .map(Vec::len)
                    .unwrap_or_default()
                    > 1
            );
        }
    }

    #[test]
    fn split_large_anchor_preserves_small_anchor_behavior() {
        let symbols = vec![
            symbol("sym-type", "Widget", "crate::Widget", SymbolKind::Struct),
            symbol(
                "sym-render",
                "render",
                "crate::Widget::render",
                SymbolKind::Method,
            ),
            symbol(
                "sym-load",
                "load",
                "crate::Widget::load",
                SymbolKind::Method,
            ),
            symbol(
                "sym-save",
                "save",
                "crate::Widget::save",
                SymbolKind::Method,
            ),
            symbol(
                "sym-sync",
                "sync",
                "crate::Widget::sync",
                SymbolKind::Method,
            ),
            symbol(
                "sym-clear",
                "clear",
                "crate::Widget::clear",
                SymbolKind::Method,
            ),
            symbol(
                "sym-helper-a",
                "helper_a",
                "crate::helpers::helper_a",
                SymbolKind::Function,
            ),
            symbol(
                "sym-helper-b",
                "helper_b",
                "crate::helpers::helper_b",
                SymbolKind::Function,
            ),
        ];
        let edges = vec![
            edge("sym-render", "sym-helper-a"),
            edge("sym-helper-a", "sym-helper-b"),
        ];

        let (assignments, _) = detect_file_communities(&edges, &symbols, &config());
        let assignment_map = assignment_map(&assignments);

        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-render")
        );
        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-load")
        );
        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-save")
        );
        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-sync")
        );
        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-clear")
        );
    }

    #[test]
    fn container_rescue_connects_same_stem_after_anchor() {
        let symbols = vec![
            symbol(
                "sym-a",
                "alpha",
                "crate::notes::alpha",
                SymbolKind::Function,
            ),
            symbol("sym-b", "beta", "crate::notes::beta", SymbolKind::Function),
        ];
        let entries = entries(&symbols);
        let (rep_by_index, rep_to_members) = rep_members(&entries);
        let mut graph = WeightedGraph::default();

        let rescued = apply_container_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            rep_to_members.as_slice(),
            &mut graph,
        );

        assert_eq!(rescued, 2);
        assert_eq!(graph.degree(rep_by_index[0]), 1);
        assert_eq!(graph.degree(rep_by_index[1]), 1);
    }

    #[test]
    fn container_rescue_skips_split_anchor_members() {
        let symbols = vec![
            symbol(
                "sym-a",
                "sir_alpha",
                "crate::SqliteStore::sir_alpha",
                SymbolKind::Method,
            ),
            symbol(
                "sym-b",
                "sir_beta",
                "crate::SqliteStore::sir_beta",
                SymbolKind::Method,
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = vec![0, 1];
        let rep_to_members = vec![vec![0], vec![1]];
        let mut graph = WeightedGraph::default();
        let split_exclusions = HashSet::from([0usize, 1usize]);

        let rescued = apply_container_rescue_with_exclusions(
            entries.as_slice(),
            rep_by_index.as_slice(),
            rep_to_members.as_slice(),
            &mut graph,
            &split_exclusions,
        );

        assert_eq!(rescued, 0);
        assert_eq!(graph.degree(0), 0);
        assert_eq!(graph.degree(1), 0);
    }

    #[test]
    fn container_rescue_still_works_for_non_split_symbols() {
        let symbols = vec![
            symbol(
                "sym-a",
                "sir_alpha",
                "crate::SqliteStore::sir_alpha",
                SymbolKind::Method,
            ),
            symbol(
                "sym-b",
                "sir_beta",
                "crate::SqliteStore::sir_beta",
                SymbolKind::Method,
            ),
            symbol(
                "sym-c",
                "sir_gamma",
                "crate::SqliteStore::sir_gamma",
                SymbolKind::Method,
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = vec![0, 1, 2];
        let rep_to_members = vec![vec![0], vec![1], vec![2]];
        let mut graph = WeightedGraph::default();
        let split_exclusions = HashSet::from([0usize]);

        let rescued = apply_container_rescue_with_exclusions(
            entries.as_slice(),
            rep_by_index.as_slice(),
            rep_to_members.as_slice(),
            &mut graph,
            &split_exclusions,
        );

        assert_eq!(rescued, 2);
        assert_eq!(graph.degree(0), 0);
        assert_eq!(graph.degree(1), 1);
        assert_eq!(graph.degree(2), 1);
    }

    #[test]
    fn semantic_rescue_connects_isolated_symbols() {
        let symbols = vec![
            with_embedding(
                symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
                &[0.98, 0.02],
            ),
            with_embedding(
                symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
                &[0.0, 1.0],
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();

        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );

        assert!(rescued >= 1);
        assert!(graph.degree(0) > 0);
        assert!(graph.degree(1) > 0);
    }

    #[test]
    fn semantic_rescue_skips_high_degree_symbols() {
        let mut graph = WeightedGraph::default();
        for target in 1..=5 {
            graph.add_edge(0, target, 1);
        }

        let mut symbols = vec![with_embedding(
            symbol("sym-hub", "hub", "crate::hub", SymbolKind::Function),
            &[1.0, 0.0],
        )];
        for index in 1..=5 {
            symbols.push(symbol(
                &format!("sym-neighbor-{index}"),
                &format!("neighbor_{index}"),
                &format!("crate::neighbor_{index}"),
                SymbolKind::Function,
            ));
        }
        symbols.push(with_embedding(
            symbol(
                "sym-source",
                "source",
                "crate::source::handler",
                SymbolKind::Function,
            ),
            &[1.0, 0.0],
        ));

        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );

        assert_eq!(rescued, 0);
        assert_eq!(graph.degree(6), 0);
    }

    #[test]
    fn semantic_rescue_respects_top_k() {
        let config = FileCommunityConfig {
            semantic_rescue_max_k: 2,
            ..config()
        };
        let symbols = vec![
            with_embedding(
                symbol(
                    "sym-source",
                    "source",
                    "crate::source",
                    SymbolKind::Function,
                ),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
                &[0.99, 0.01],
            ),
            with_embedding(
                symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
                &[0.98, 0.02],
            ),
            with_embedding(
                symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
                &[0.97, 0.03],
            ),
            with_embedding(
                symbol("sym-d", "delta", "crate::delta", SymbolKind::Function),
                &[0.96, 0.04],
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();

        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config,
        );

        assert!(rescued >= 1);
        assert_eq!(graph.neighbors(0).len(), 2);
    }

    #[test]
    fn loners_excluded_from_output() {
        let symbols = vec![
            symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
            symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
            symbol(
                "sym-c",
                "gamma",
                "crate::gamma::handler",
                SymbolKind::Function,
            ),
        ];
        let (assignments, diagnostics) =
            detect_file_communities(&[edge("sym-a", "sym-b")], &symbols, &config());

        let assignment_map = assignment_map(&assignments);
        assert!(assignment_map.contains_key("sym-a"));
        assert!(assignment_map.contains_key("sym-b"));
        assert!(!assignment_map.contains_key("sym-c"));
        assert_eq!(diagnostics.symbols_loner, 1);
    }

    #[test]
    fn merge_pass_absorbs_small_communities() {
        let entries = entries(&[
            symbol("a1", "a1", "crate::a1", SymbolKind::Function),
            symbol("a2", "a2", "crate::a2", SymbolKind::Function),
            symbol("a3", "a3", "crate::a3", SymbolKind::Function),
            symbol("s1", "s1", "crate::s1", SymbolKind::Function),
            symbol("s2", "s2", "crate::s2", SymbolKind::Function),
        ]);
        let rep_to_members = vec![vec![0], vec![1], vec![2], vec![3], vec![4]];
        let rep_to_community = HashMap::from([(0, 1), (1, 1), (2, 1), (3, 2), (4, 2)]);
        let component_of_rep = HashMap::from([(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)]);
        let mut structural_graph = WeightedGraph::default();
        structural_graph.add_edge(3, 0, 1);
        structural_graph.add_edge(4, 1, 1);

        let (merged, penalty) = merge_small_communities(
            rep_to_community,
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &structural_graph,
            3,
        );

        assert_eq!(penalty, 0.0);
        assert!(merged.values().all(|community_id| *community_id == 1));
    }

    #[test]
    fn merge_pass_respects_component_boundaries() {
        let entries = entries(&[
            symbol("a1", "a1", "crate::a1", SymbolKind::Function),
            symbol("a2", "a2", "crate::a2", SymbolKind::Function),
            symbol("a3", "a3", "crate::a3", SymbolKind::Function),
            symbol("s1", "s1", "crate::s1", SymbolKind::Function),
            symbol("s2", "s2", "crate::s2", SymbolKind::Function),
        ]);
        let rep_to_members = vec![vec![0], vec![1], vec![2], vec![3], vec![4]];
        let rep_to_community = HashMap::from([(0, 1), (1, 1), (2, 1), (3, 2), (4, 2)]);
        let component_of_rep = HashMap::from([(0, 0), (1, 0), (2, 0), (3, 1), (4, 1)]);

        let (merged, penalty) = merge_small_communities(
            rep_to_community,
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &WeightedGraph::default(),
            3,
        );

        assert_eq!(merged.get(&3), Some(&2));
        assert_eq!(merged.get(&4), Some(&2));
        assert_eq!(penalty, 0.0);
    }

    #[test]
    fn merge_tiebreak_is_deterministic() {
        let entries = entries(&[
            symbol("a1", "a1", "crate::a1", SymbolKind::Function),
            symbol("a2", "a2", "crate::a2", SymbolKind::Function),
            symbol("a3", "a3", "crate::a3", SymbolKind::Function),
            symbol("b1", "b1", "crate::b1", SymbolKind::Function),
            symbol("b2", "b2", "crate::b2", SymbolKind::Function),
            symbol("b3", "b3", "crate::b3", SymbolKind::Function),
            symbol("s1", "s1", "crate::s1", SymbolKind::Function),
            symbol("s2", "s2", "crate::s2", SymbolKind::Function),
        ]);
        let rep_to_members = vec![
            vec![0],
            vec![1],
            vec![2],
            vec![3],
            vec![4],
            vec![5],
            vec![6],
            vec![7],
        ];
        let rep_to_community = HashMap::from([
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 2),
            (6, 3),
            (7, 3),
        ]);
        let component_of_rep = (0..8).map(|rep| (rep, 0usize)).collect::<HashMap<_, _>>();
        let mut structural_graph = WeightedGraph::default();
        structural_graph.add_edge(6, 0, 1);
        structural_graph.add_edge(7, 3, 1);

        let (merged, penalty) = merge_small_communities(
            rep_to_community,
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &structural_graph,
            3,
        );

        assert_eq!(penalty, 0.0);
        assert_eq!(merged.get(&6), Some(&1));
        assert_eq!(merged.get(&7), Some(&1));
    }

    #[test]
    fn merge_fallback_uses_semantic_when_no_structural_winner() {
        let entries = entries(&[
            with_embedding(
                symbol("a1", "a1", "crate::a1", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol("a2", "a2", "crate::a2", SymbolKind::Function),
                &[0.95, 0.05],
            ),
            with_embedding(
                symbol("a3", "a3", "crate::a3", SymbolKind::Function),
                &[0.9, 0.1],
            ),
            with_embedding(
                symbol("b1", "b1", "crate::b1", SymbolKind::Function),
                &[0.0, 1.0],
            ),
            with_embedding(
                symbol("b2", "b2", "crate::b2", SymbolKind::Function),
                &[0.1, 0.9],
            ),
            with_embedding(
                symbol("b3", "b3", "crate::b3", SymbolKind::Function),
                &[0.05, 0.95],
            ),
            with_embedding(
                symbol("s1", "s1", "crate::s1", SymbolKind::Function),
                &[0.05, 0.95],
            ),
            with_embedding(
                symbol("s2", "s2", "crate::s2", SymbolKind::Function),
                &[0.02, 0.98],
            ),
        ]);
        let rep_to_members = vec![
            vec![0],
            vec![1],
            vec![2],
            vec![3],
            vec![4],
            vec![5],
            vec![6],
            vec![7],
        ];
        let rep_to_community = HashMap::from([
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 2),
            (4, 2),
            (5, 2),
            (6, 3),
            (7, 3),
        ]);
        let component_of_rep = (0..8).map(|rep| (rep, 0usize)).collect::<HashMap<_, _>>();

        let (merged, penalty) = merge_small_communities(
            rep_to_community,
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &WeightedGraph::default(),
            3,
        );

        assert_eq!(penalty, 0.0);
        assert_eq!(merged.get(&6), Some(&2));
        assert_eq!(merged.get(&7), Some(&2));
    }

    #[test]
    fn merge_fallback_leaves_unmerged_when_no_signal() {
        let entries = entries(&[
            symbol("a1", "a1", "crate::a1", SymbolKind::Function),
            symbol("a2", "a2", "crate::a2", SymbolKind::Function),
            symbol("a3", "a3", "crate::a3", SymbolKind::Function),
            symbol("s1", "s1", "crate::s1", SymbolKind::Function),
            symbol("s2", "s2", "crate::s2", SymbolKind::Function),
        ]);
        let rep_to_members = vec![vec![0], vec![1], vec![2], vec![3], vec![4]];
        let rep_to_community = HashMap::from([(0, 1), (1, 1), (2, 1), (3, 2), (4, 2)]);
        let component_of_rep = (0..5).map(|rep| (rep, 0usize)).collect::<HashMap<_, _>>();

        let (merged, penalty) = merge_small_communities(
            rep_to_community.clone(),
            entries.as_slice(),
            rep_to_members.as_slice(),
            &component_of_rep,
            &WeightedGraph::default(),
            3,
        );

        assert_eq!(merged, rep_to_community);
        assert!((penalty - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn stability_check_detects_unstable_partition() {
        let symbols = vec![
            with_embedding(
                symbol(
                    "sym-a",
                    "alpha",
                    "crate::alpha::handler",
                    SymbolKind::Function,
                ),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol(
                    "sym-b",
                    "beta",
                    "crate::beta::handler",
                    SymbolKind::Function,
                ),
                &[0.74, 0.6726],
            ),
            symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
            symbol("sym-d", "delta", "crate::delta", SymbolKind::Function),
        ];
        let edges = vec![edge("sym-c", "sym-d")];

        let (_, diagnostics) = detect_file_communities(&edges, &symbols, &config());
        assert!(diagnostics.stability_score < 0.8);
    }

    #[test]
    fn full_pipeline_produces_actionable_groups() {
        let symbols = vec![
            with_embedding(
                symbol("sym-type", "Widget", "crate::Widget", SymbolKind::Struct),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol(
                    "sym-method",
                    "render",
                    "crate::Widget::render",
                    SymbolKind::Method,
                ),
                &[1.0, 0.0],
            ),
            with_embedding(
                symbol(
                    "sym-helper",
                    "helper",
                    "crate::helper",
                    SymbolKind::Function,
                ),
                &[0.95, 0.05],
            ),
            with_embedding(
                symbol(
                    "sym-note-a",
                    "note_alpha",
                    "crate::notes::alpha",
                    SymbolKind::Function,
                ),
                &[0.0, 1.0],
            ),
            with_embedding(
                symbol(
                    "sym-note-b",
                    "note_beta",
                    "crate::notes::beta",
                    SymbolKind::Function,
                ),
                &[0.05, 0.95],
            ),
            with_embedding(
                symbol(
                    "sym-note-c",
                    "note_gamma",
                    "crate::notes::gamma",
                    SymbolKind::Function,
                ),
                &[0.08, 0.92],
            ),
        ];
        let edges = vec![
            edge("sym-method", "sym-helper"),
            edge("sym-note-a", "sym-note-b"),
            edge("sym-note-b", "sym-note-c"),
        ];

        let (assignments, diagnostics) = detect_file_communities(&edges, &symbols, &config());
        let assignment_map = assignment_map(&assignments);

        assert_eq!(community_ids(&assignments).len(), 2);
        assert_eq!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-method")
        );
        assert_eq!(
            assignment_map.get("sym-note-a"),
            assignment_map.get("sym-note-b")
        );
        assert_eq!(
            assignment_map.get("sym-note-b"),
            assignment_map.get("sym-note-c")
        );
        assert_ne!(
            assignment_map.get("sym-type"),
            assignment_map.get("sym-note-a")
        );
        assert!(diagnostics.confidence > 0.0);
    }

    #[test]
    #[ignore]
    fn ablation_aether_store() {
        run_ablation_report("aether-store");
    }

    #[test]
    #[ignore]
    fn ablation_aether_config() {
        run_ablation_report("aether-config");
    }

    #[test]
    #[ignore]
    fn ablation_aether_mcp() {
        run_ablation_report("aether-mcp");
    }

    fn run_ablation_report(crate_name: &str) {
        let Some(input) = load_ablation_input(crate_name) else {
            return;
        };
        let variants = [
            (
                "1. baseline",
                AblationOptions {
                    filter_tests: false,
                    type_anchor: false,
                    container_rescue: false,
                    semantic_rescue: false,
                    community_resolution: 1.0,
                    merge_small: false,
                },
            ),
            (
                "2. + test filtering",
                AblationOptions {
                    filter_tests: true,
                    type_anchor: false,
                    container_rescue: false,
                    semantic_rescue: false,
                    community_resolution: 1.0,
                    merge_small: false,
                },
            ),
            (
                "3. + type-anchor",
                AblationOptions {
                    filter_tests: true,
                    type_anchor: true,
                    container_rescue: false,
                    semantic_rescue: false,
                    community_resolution: 1.0,
                    merge_small: false,
                },
            ),
            (
                "4. + rescue",
                AblationOptions {
                    filter_tests: true,
                    type_anchor: true,
                    container_rescue: true,
                    semantic_rescue: true,
                    community_resolution: 1.0,
                    merge_small: false,
                },
            ),
            (
                "5. + lower gamma",
                AblationOptions {
                    filter_tests: true,
                    type_anchor: true,
                    container_rescue: true,
                    semantic_rescue: true,
                    community_resolution: 0.5,
                    merge_small: false,
                },
            ),
            (
                "6. full pipeline",
                AblationOptions {
                    filter_tests: true,
                    type_anchor: true,
                    container_rescue: true,
                    semantic_rescue: true,
                    community_resolution: input.config.community_resolution,
                    merge_small: true,
                },
            ),
        ];

        println!("\nAblation for {} ({})", input.crate_name, input.file_path);
        println!(
            "{:<28} {:>11} {:>8} {:>9} {:>8} {:>8} {:>10} {}",
            "configuration",
            "communities",
            "largest",
            "smallest",
            "loners",
            "conf",
            "stability",
            "top modules"
        );

        for (label, options) in variants {
            let (assignments, diagnostics) =
                run_ablation_detection(&input.edges, &input.symbols, &input.config, options);
            let sizes = community_sizes(&assignments);
            let largest = sizes.iter().copied().max().unwrap_or(0);
            let smallest = sizes.iter().copied().min().unwrap_or(0);
            let top_modules = ablation_module_names(&input.symbols, &assignments)
                .into_iter()
                .take(3)
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{:<28} {:>11} {:>8} {:>9} {:>8} {:>8.2} {:>10.2} {}",
                label,
                sizes.len(),
                largest,
                smallest,
                diagnostics.symbols_loner,
                diagnostics.confidence,
                diagnostics.stability_score,
                if top_modules.is_empty() {
                    "-".to_owned()
                } else {
                    top_modules
                }
            );
        }
    }

    fn load_ablation_input(crate_name: &str) -> Option<AblationInput> {
        let workspace = std::path::Path::new("/home/rephu/projects/aether");
        let aether_dir = workspace.join(".aether");
        if !aether_dir.exists() {
            eprintln!(
                "ablation skipped for {crate_name}: {} is missing",
                aether_dir.display()
            );
            return None;
        }

        let config = match load_workspace_config(workspace) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to load config: {err}");
                return None;
            }
        };
        let Some(model_name) = config.embeddings.model.clone() else {
            eprintln!("ablation skipped for {crate_name}: embeddings.model is not configured");
            return None;
        };

        let file_path = format!("crates/{crate_name}/src/lib.rs");
        let store = match SqliteStore::open_readonly(workspace) {
            Ok(store) => store,
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to open sqlite store: {err}");
                return None;
            }
        };
        let symbol_records = match store.list_symbols_for_file(file_path.as_str()) {
            Ok(records) if !records.is_empty() => records,
            Ok(_) => {
                eprintln!("ablation skipped for {crate_name}: no indexed symbols for {file_path}");
                return None;
            }
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to load symbols: {err}");
                return None;
            }
        };

        let symbol_ids = symbol_records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        let symbol_id_set = symbol_ids
            .iter()
            .map(|symbol_id| symbol_id.as_str())
            .collect::<HashSet<_>>();

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to build runtime: {err}");
                return None;
            }
        };

        let graph = match runtime.block_on(SurrealGraphStore::open_readonly(workspace)) {
            Ok(graph) => graph,
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to open surreal graph: {err}");
                return None;
            }
        };
        let edges = match runtime.block_on(graph.list_dependency_edges()) {
            Ok(records) => records
                .into_iter()
                .filter(|edge| {
                    symbol_id_set.contains(edge.source_symbol_id.as_str())
                        && symbol_id_set.contains(edge.target_symbol_id.as_str())
                })
                .map(|edge| GraphAlgorithmEdge {
                    source_id: edge.source_symbol_id,
                    target_id: edge.target_symbol_id,
                    edge_kind: edge.edge_kind,
                })
                .collect::<Vec<_>>(),
            Err(err) => {
                eprintln!(
                    "ablation skipped for {crate_name}: failed to load dependency edges: {err}"
                );
                return None;
            }
        };
        let vector_store = match runtime.block_on(open_vector_store(workspace)) {
            Ok(store) => store,
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to open vector store: {err}");
                return None;
            }
        };
        let embedding_by_id = match runtime.block_on(vector_store.list_embeddings_for_symbols(
            config.embeddings.provider.as_str(),
            model_name.as_str(),
            symbol_ids.as_slice(),
        )) {
            Ok(records) => records
                .into_iter()
                .map(|record| (record.symbol_id, record.embedding))
                .collect::<HashMap<_, _>>(),
            Err(err) => {
                eprintln!("ablation skipped for {crate_name}: failed to load embeddings: {err}");
                return None;
            }
        };

        let symbols = symbol_records
            .into_iter()
            .map(|record| FileSymbol {
                symbol_id: record.id.clone(),
                name: ablation_symbol_name(record.qualified_name.as_str()),
                qualified_name: record.qualified_name.clone(),
                kind: ablation_parse_symbol_kind(record.kind.as_str()),
                is_test: ablation_is_test(
                    record.qualified_name.as_str(),
                    record.file_path.as_str(),
                ),
                embedding: embedding_by_id.get(record.id.as_str()).cloned(),
            })
            .collect::<Vec<_>>();

        Some(AblationInput {
            crate_name: crate_name.to_owned(),
            file_path,
            symbols,
            edges,
            config: FileCommunityConfig {
                semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
                semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
                community_resolution: config.planner.community_resolution,
                min_community_size: config.planner.min_community_size,
            },
        })
    }

    fn run_ablation_detection(
        structural_edges: &[GraphAlgorithmEdge],
        symbols: &[FileSymbol],
        config: &FileCommunityConfig,
        options: AblationOptions,
    ) -> (Vec<(String, usize)>, PlannerDiagnostics) {
        let baseline = run_ablation_pass(structural_edges, symbols, config, options);
        if baseline.non_test_count == 0 || baseline.assignments.is_empty() {
            let diagnostics = diagnostics_from_run(&baseline, 0.0, 0.0);
            return (baseline.assignments, diagnostics);
        }

        let mut threshold_config = config.clone();
        threshold_config.semantic_rescue_threshold =
            (threshold_config.semantic_rescue_threshold + 0.05).clamp(0.3, 0.95);
        let threshold_run =
            run_ablation_pass(structural_edges, symbols, &threshold_config, options);

        let mut resolution_config = config.clone();
        resolution_config.community_resolution =
            (options.community_resolution + 0.1).clamp(0.1, 3.0);
        let resolution_options = AblationOptions {
            community_resolution: resolution_config.community_resolution,
            ..options
        };
        let resolution_run = run_ablation_pass(
            structural_edges,
            symbols,
            &resolution_config,
            resolution_options,
        );

        let stability_score = pairwise_jaccard(
            baseline.assignments.as_slice(),
            threshold_run.assignments.as_slice(),
        )
        .min(pairwise_jaccard(
            baseline.assignments.as_slice(),
            resolution_run.assignments.as_slice(),
        ));
        let confidence = compute_confidence(&baseline, stability_score);
        let diagnostics = diagnostics_from_run(&baseline, stability_score, confidence);
        (baseline.assignments, diagnostics)
    }

    fn run_ablation_pass(
        structural_edges: &[GraphAlgorithmEdge],
        symbols: &[FileSymbol],
        config: &FileCommunityConfig,
        options: AblationOptions,
    ) -> DetectionRun {
        let symbols_total = symbols.len();
        let mut filtered_symbols = if options.filter_tests {
            symbols
                .iter()
                .filter(|symbol| !symbol.is_test)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            symbols.to_vec()
        };
        filtered_symbols.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));
        let symbols_filtered_test = if options.filter_tests {
            symbols_total.saturating_sub(filtered_symbols.len())
        } else {
            0
        };
        let non_test_count = filtered_symbols.len();

        let entries = filtered_symbols
            .into_iter()
            .map(|symbol| SymbolEntry {
                stem: qualified_name_stem(symbol.qualified_name.as_str()),
                symbol,
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return DetectionRun {
                assignments: Vec::new(),
                symbols_total,
                symbols_filtered_test,
                symbols_anchored_type: 0,
                symbols_rescued_container: 0,
                symbols_rescued_semantic: 0,
                symbols_loner: 0,
                communities_before_merge: 0,
                communities_after_merge: 0,
                embedding_coverage_pct: 0.0,
                non_test_count: 0,
                unmerged_small_penalty: 0.0,
            };
        }

        let id_to_index = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.symbol.symbol_id.clone(), index))
            .collect::<HashMap<_, _>>();
        let filtered_structural_edges = structural_edges
            .iter()
            .filter(|edge| {
                id_to_index.contains_key(edge.source_id.as_str())
                    && id_to_index.contains_key(edge.target_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();

        let (mut union_find, initial_anchor_groups, split_anchor_exclusions) =
            if options.type_anchor {
                let (_anchor_union_find, anchor_groups) = build_anchor_groups(entries.as_slice());
                let (anchor_groups, split_anchor_exclusions) =
                    split_large_anchor_groups(entries.as_slice(), anchor_groups);
                let union_find =
                    rebuild_union_find_from_groups(entries.len(), anchor_groups.as_slice());
                (union_find, anchor_groups, split_anchor_exclusions)
            } else {
                let mut union_find = DisjointSet::new(entries.len());
                let anchor_groups = build_rep_members(entries.len(), &mut union_find);
                (union_find, anchor_groups, HashSet::new())
            };
        let symbols_anchored_type = if options.type_anchor {
            count_type_anchored_symbols(entries.as_slice(), initial_anchor_groups.as_slice())
        } else {
            0
        };
        let rep_to_members_diag = build_rep_members(entries.len(), &mut union_find);
        let (nc, nl) =
            count_components_and_largest(&WeightedGraph::default(), &rep_to_members_diag, &entries);
        eprintln!(
            "[diag] after_anchor_split: groups={} largest_group={}",
            nc, nl
        );

        let structural_graph = collapse_structural_edges(
            filtered_structural_edges.as_slice(),
            &id_to_index,
            &mut union_find,
        );
        let mut enriched_graph = structural_graph.clone();
        let rep_to_members = build_rep_members(entries.len(), &mut union_find);
        let rep_by_index = (0..entries.len())
            .map(|index| union_find.find(index))
            .collect::<Vec<_>>();
        let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
        eprintln!(
            "[diag] after_structural_edges: components={} largest_component={}",
            nc, nl
        );

        let symbols_rescued_container = if options.container_rescue {
            apply_container_rescue_with_exclusions(
                entries.as_slice(),
                rep_by_index.as_slice(),
                rep_to_members.as_slice(),
                &mut enriched_graph,
                &split_anchor_exclusions,
            )
        } else {
            0
        };
        let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
        eprintln!(
            "[diag] after_container_rescue: components={} largest_component={} rescued={}",
            nc, nl, symbols_rescued_container
        );
        let symbols_rescued_semantic = if options.semantic_rescue {
            apply_semantic_rescue(
                entries.as_slice(),
                rep_by_index.as_slice(),
                &mut enriched_graph,
                config,
            )
        } else {
            0
        };
        let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
        eprintln!(
            "[diag] after_semantic_rescue: components={} largest_component={} rescued={}",
            nc, nl, symbols_rescued_semantic
        );

        let loner_reps = rep_to_members
            .iter()
            .enumerate()
            .filter_map(|(rep, members)| {
                if members.is_empty() || enriched_graph.degree(rep) > 0 {
                    None
                } else {
                    Some(rep)
                }
            })
            .collect::<HashSet<_>>();
        let symbols_loner = loner_reps
            .iter()
            .filter_map(|rep| rep_to_members.get(*rep))
            .map(Vec::len)
            .sum();
        let embedding_coverage_pct = if non_test_count == 0 {
            0.0
        } else {
            entries
                .iter()
                .filter(|entry| has_embedding(entry.symbol.embedding.as_deref()))
                .count() as f32
                / non_test_count as f32
        };

        let active_reps = rep_to_members
            .iter()
            .enumerate()
            .filter_map(|(rep, members)| {
                if members.is_empty() || loner_reps.contains(&rep) {
                    None
                } else {
                    Some(rep)
                }
            })
            .collect::<Vec<_>>();
        if active_reps.is_empty() {
            return DetectionRun {
                assignments: Vec::new(),
                symbols_total,
                symbols_filtered_test,
                symbols_anchored_type,
                symbols_rescued_container,
                symbols_rescued_semantic,
                symbols_loner,
                communities_before_merge: 0,
                communities_after_merge: 0,
                embedding_coverage_pct,
                non_test_count,
                unmerged_small_penalty: 0.0,
            };
        }

        let components =
            enriched_graph.connected_components(active_reps.as_slice(), entries.as_slice());
        let component_sizes: Vec<usize> = components.iter().map(Vec::len).collect();
        eprintln!(
            "[diag] connected_components: count={} sizes={:?}",
            components.len(),
            component_sizes
        );
        let component_of_rep = components
            .iter()
            .enumerate()
            .flat_map(|(component_id, reps)| {
                reps.iter().copied().map(move |rep| (rep, component_id))
            })
            .collect::<HashMap<_, _>>();
        let mut rep_to_community = HashMap::<usize, usize>::new();
        let mut next_community_id = 1usize;
        for component in &components {
            let local_edges = enriched_graph.repeated_component_edges(component.as_slice());
            let local_assignments = aether_graph_algo::louvain_with_resolution_sync(
                local_edges.as_slice(),
                options.community_resolution,
            );
            let mut local_to_global = BTreeMap::<usize, usize>::new();
            for (_, local_id) in &local_assignments {
                local_to_global.entry(*local_id).or_insert_with(|| {
                    let assigned = next_community_id;
                    next_community_id += 1;
                    assigned
                });
            }

            for (rep_name, local_id) in local_assignments {
                let Some(rep) = rep_name
                    .strip_prefix("rep-")
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    continue;
                };
                if let Some(global_id) = local_to_global.get(&local_id).copied() {
                    rep_to_community.insert(rep, global_id);
                }
            }
        }
        let communities_before_merge = rep_to_community
            .values()
            .copied()
            .collect::<HashSet<_>>()
            .len();
        eprintln!(
            "[diag] after_louvain: communities={}",
            communities_before_merge
        );

        let (rep_to_community, unmerged_small_penalty, communities_after_merge) =
            if options.merge_small {
                let (merged, penalty) = merge_small_communities(
                    rep_to_community,
                    entries.as_slice(),
                    rep_to_members.as_slice(),
                    &component_of_rep,
                    &structural_graph,
                    config.min_community_size,
                );
                let count = merged.values().copied().collect::<HashSet<_>>().len();
                (merged, penalty, count)
            } else {
                (rep_to_community, 0.0, communities_before_merge)
            };

        let assignments = finalize_assignments(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &loner_reps,
            &rep_to_community,
        );

        DetectionRun {
            assignments,
            symbols_total,
            symbols_filtered_test,
            symbols_anchored_type,
            symbols_rescued_container,
            symbols_rescued_semantic,
            symbols_loner,
            communities_before_merge,
            communities_after_merge,
            embedding_coverage_pct,
            non_test_count,
            unmerged_small_penalty,
        }
    }

    fn community_sizes(assignments: &[(String, usize)]) -> Vec<usize> {
        let mut sizes = BTreeMap::<usize, usize>::new();
        for (_, community_id) in assignments {
            *sizes.entry(*community_id).or_default() += 1;
        }
        sizes.into_values().collect()
    }

    fn ablation_module_names(
        symbols: &[FileSymbol],
        assignments: &[(String, usize)],
    ) -> Vec<String> {
        let symbol_by_id = symbols
            .iter()
            .map(|symbol| (symbol.symbol_id.as_str(), symbol))
            .collect::<HashMap<_, _>>();
        let mut grouped = BTreeMap::<usize, Vec<String>>::new();
        for (symbol_id, community_id) in assignments {
            if let Some(symbol) = symbol_by_id.get(symbol_id.as_str()) {
                grouped
                    .entry(*community_id)
                    .or_default()
                    .push(symbol.name.clone());
            }
        }

        let mut seen = BTreeSet::new();
        grouped
            .into_iter()
            .map(|(community_id, names)| {
                let mut counts = BTreeMap::<String, usize>::new();
                for name in names {
                    for token in name.to_ascii_lowercase().split('_') {
                        if token.is_empty() {
                            continue;
                        }
                        let normalized = match token {
                            "note" | "notes" => "note",
                            "migration" | "migrate" => "migration",
                            "test" | "tests" => "test",
                            "store" | "stores" => "store",
                            other => other,
                        };
                        if ABLATION_STOPWORDS.contains(&normalized) {
                            continue;
                        }
                        *counts.entry(normalized.to_owned()).or_default() += 1;
                    }
                }
                let mut ranked = counts.into_iter().collect::<Vec<_>>();
                ranked
                    .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
                let base = ranked
                    .first()
                    .map(|(token, _)| token.clone())
                    .unwrap_or_else(|| format!("community_{community_id}"));
                let name = format!("{base}_ops");
                if seen.insert(name.clone()) {
                    name
                } else {
                    format!("{base}_{community_id}_ops")
                }
            })
            .collect()
    }

    fn ablation_symbol_name(qualified_name: &str) -> String {
        qualified_name
            .rsplit("::")
            .next()
            .unwrap_or(qualified_name)
            .trim_start_matches("r#")
            .to_owned()
    }

    fn ablation_parse_symbol_kind(raw: &str) -> SymbolKind {
        match raw.trim().to_ascii_lowercase().as_str() {
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "variable" => SymbolKind::Variable,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "trait" => SymbolKind::Trait,
            "interface" => SymbolKind::Interface,
            "type_alias" | "typealias" => SymbolKind::TypeAlias,
            _ => SymbolKind::Function,
        }
    }

    fn ablation_is_test(qualified_name: &str, file_path: &str) -> bool {
        let leaf = ablation_symbol_name(qualified_name).to_ascii_lowercase();
        leaf.starts_with("test_")
            || file_path.contains("/tests/")
            || file_path.contains("\\tests\\")
    }
}
