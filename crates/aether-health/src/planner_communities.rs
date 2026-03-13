use std::collections::{BTreeMap, HashMap, HashSet};

use aether_core::SymbolKind;
use aether_graph_algo::{GraphAlgorithmEdge, louvain_with_resolution_sync};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod ablation;
mod anchors;
mod graph;
mod merge;
mod rescue;

use self::anchors::{
    build_anchor_groups, count_type_anchored_symbols, rebuild_union_find_from_groups,
    split_large_anchor_groups,
};
use self::graph::{build_rep_members, collapse_structural_edges};
use self::merge::{finalize_assignments, merge_small_communities};
use self::rescue::{apply_container_rescue_with_exclusions, apply_semantic_rescue, has_embedding};

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

fn qualified_name_stem(value: &str) -> String {
    value
        .rsplit_once("::")
        .map(|(stem, _)| stem.to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use aether_core::SymbolKind;
    use aether_graph_algo::GraphAlgorithmEdge;

    use super::{
        FileCommunityConfig, FileSymbol, confidence_label, detect_file_communities,
        pairwise_jaccard,
    };

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
    fn semantic_rescue_stability_under_threshold_perturbation() {
        let symbols = vec![
            symbol("sym-a0", "a0", "crate::a0", SymbolKind::Function),
            with_embedding(
                symbol("sym-a1", "a1", "crate::a1", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            symbol("sym-a2", "a2", "crate::a2", SymbolKind::Function),
            symbol("sym-b0", "b0", "crate::b0", SymbolKind::Function),
            with_embedding(
                symbol("sym-b1", "b1", "crate::b1", SymbolKind::Function),
                &[0.0, 1.0],
            ),
            symbol("sym-b2", "b2", "crate::b2", SymbolKind::Function),
            with_embedding(
                symbol(
                    "sym-orphan",
                    "orphan",
                    "crate::orphan",
                    SymbolKind::Function,
                ),
                &[0.98, 0.02],
            ),
        ];
        let edges = vec![
            edge("sym-a0", "sym-a1"),
            edge("sym-a1", "sym-a2"),
            edge("sym-b0", "sym-b1"),
            edge("sym-b1", "sym-b2"),
        ];
        let baseline = detect_file_communities(&edges, &symbols, &config()).0;
        let mut threshold_config = config();
        threshold_config.semantic_rescue_threshold =
            (threshold_config.semantic_rescue_threshold + 0.05).clamp(0.3, 0.95);
        let perturbed = detect_file_communities(&edges, &symbols, &threshold_config).0;

        assert!(pairwise_jaccard(baseline.as_slice(), perturbed.as_slice()) >= 0.90);
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
}
