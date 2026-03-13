use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

use super::graph::{WeightedGraph, normalized_pair};
use super::{FileCommunityConfig, SymbolEntry};

#[derive(Clone)]
struct SemanticCandidate {
    similarity: f32,
    symbol_id: String,
    target_rep: usize,
    target_component_size: usize,
}

#[allow(dead_code)]
pub(super) fn apply_container_rescue(
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

pub(super) fn apply_container_rescue_with_exclusions(
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

pub(super) fn apply_semantic_rescue(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    graph: &mut WeightedGraph,
    config: &FileCommunityConfig,
) -> usize {
    let active_reps = rep_by_index
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if active_reps.is_empty() {
        return 0;
    }

    let components = graph.connected_components(active_reps.as_slice(), entries);
    let mut component_of_rep = HashMap::new();
    let mut component_size_by_id = HashMap::new();
    for (component_id, reps) in components.iter().enumerate() {
        component_size_by_id.insert(component_id, reps.len());
        for rep in reps {
            component_of_rep.insert(*rep, component_id);
        }
    }

    let source_indices = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            has_embedding(entry.symbol.embedding.as_deref()).then_some(index)
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
        let Some(source_component_id) = component_of_rep.get(&source_rep).copied() else {
            continue;
        };
        let source_component_size = component_size_by_id
            .get(&source_component_id)
            .copied()
            .unwrap_or(1);
        let source_degree = graph.degree(source_rep);
        if source_component_size > 1 {
            if source_degree > 1 {
                continue;
            }
        } else if source_degree > 0 {
            continue;
        }

        let mut candidates_by_rep = HashMap::<usize, SemanticCandidate>::new();
        for (target_index, target) in entries.iter().enumerate() {
            if source_index == target_index {
                continue;
            }
            let target_rep = rep_by_index
                .get(target_index)
                .copied()
                .unwrap_or(target_index);
            if target_rep == source_rep || graph.has_edge(source_rep, target_rep) {
                continue;
            }
            if graph.degree(target_rep) >= 5 {
                continue;
            }
            let Some(target_component_id) = component_of_rep.get(&target_rep).copied() else {
                continue;
            };
            if source_component_size > 1 && target_component_id != source_component_id {
                continue;
            }
            let Some(similarity) =
                cosine_similarity(Some(source_embedding), target.symbol.embedding.as_deref())
            else {
                continue;
            };
            let candidate = SemanticCandidate {
                similarity,
                symbol_id: target.symbol.symbol_id.clone(),
                target_rep,
                target_component_size: component_size_by_id
                    .get(&target_component_id)
                    .copied()
                    .unwrap_or(1),
            };
            let replace = match candidates_by_rep.get(&target_rep) {
                Some(existing) => sort_semantic_candidates(&candidate, existing) == Ordering::Less,
                None => true,
            };
            if replace {
                candidates_by_rep.insert(target_rep, candidate);
            }
        }

        let mut candidates = candidates_by_rep.into_values().collect::<Vec<_>>();
        candidates.sort_by(sort_semantic_candidates);
        if source_component_size > 1 {
            let Some(best_similarity) = candidates.first().map(|candidate| candidate.similarity)
            else {
                continue;
            };
            if best_similarity < config.semantic_rescue_threshold {
                continue;
            }

            let mut added = false;
            for candidate in candidates.into_iter().take(config.semantic_rescue_max_k) {
                if graph.has_edge(source_rep, candidate.target_rep) {
                    continue;
                }
                graph.add_edge(source_rep, candidate.target_rep, 1);
                added = true;
            }
            if added {
                rescued.insert(source_index);
            }
            continue;
        }

        let best_non_singleton = candidates
            .iter()
            .find(|candidate| {
                candidate.target_component_size > 1
                    && candidate.similarity >= config.semantic_rescue_threshold
            })
            .cloned();
        let best_singleton = candidates
            .iter()
            .find(|candidate| {
                candidate.target_component_size == 1
                    && candidate.similarity >= config.semantic_rescue_threshold
            })
            .cloned();
        let Some(candidate) = best_non_singleton.or(best_singleton) else {
            continue;
        };
        if !graph.has_edge(source_rep, candidate.target_rep) {
            graph.add_edge(source_rep, candidate.target_rep, 1);
            rescued.insert(source_index);
        }
    }

    rescued.len()
}

pub(super) fn has_embedding(embedding: Option<&[f32]>) -> bool {
    embedding.is_some_and(|values| {
        !values.is_empty()
            && values.iter().all(|value| value.is_finite())
            && values.iter().map(|value| value * value).sum::<f32>() > f32::EPSILON
    })
}

pub(super) fn cosine_similarity(left: Option<&[f32]>, right: Option<&[f32]>) -> Option<f32> {
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

fn sort_semantic_candidates(left: &SemanticCandidate, right: &SemanticCandidate) -> Ordering {
    right
        .similarity
        .partial_cmp(&left.similarity)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.symbol_id.as_str().cmp(right.symbol_id.as_str()))
        .then_with(|| left.target_rep.cmp(&right.target_rep))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use aether_core::SymbolKind;

    use super::super::anchors::build_anchor_groups;
    use super::super::graph::{WeightedGraph, build_rep_members};
    use super::super::{FileCommunityConfig, FileSymbol, SymbolEntry, qualified_name_stem};
    use super::{
        apply_container_rescue, apply_container_rescue_with_exclusions, apply_semantic_rescue,
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

        assert_eq!(rescued, 1);
        assert_eq!(graph.degree(0), 1);
        assert_eq!(graph.degree(1), 1);
        assert_eq!(graph.degree(2), 0);
        assert!(graph.has_edge(0, 1));
    }

    #[test]
    fn semantic_rescue_skips_high_degree_symbols() {
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(0, 2, 1);
        graph.add_edge(3, 4, 1);
        graph.add_edge(3, 5, 1);

        let symbols = vec![
            with_embedding(
                symbol("sym-hub", "hub", "crate::hub", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            symbol(
                "sym-neighbor-1",
                "neighbor_1",
                "crate::neighbor_1",
                SymbolKind::Function,
            ),
            symbol(
                "sym-neighbor-2",
                "neighbor_2",
                "crate::neighbor_2",
                SymbolKind::Function,
            ),
            with_embedding(
                symbol(
                    "sym-source",
                    "source",
                    "crate::source::handler",
                    SymbolKind::Function,
                ),
                &[0.99, 0.01],
            ),
            symbol(
                "sym-neighbor-4",
                "neighbor_4",
                "crate::neighbor_4",
                SymbolKind::Function,
            ),
            symbol(
                "sym-neighbor-5",
                "neighbor_5",
                "crate::neighbor_5",
                SymbolKind::Function,
            ),
        ];

        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );

        assert_eq!(rescued, 0);
        assert_eq!(graph.degree(0), 2);
        assert_eq!(graph.degree(3), 2);
        assert!(!graph.has_edge(0, 3));
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
            symbol("sym-a", "alpha", "crate::alpha", SymbolKind::Function),
            with_embedding(
                symbol("sym-b", "beta", "crate::beta", SymbolKind::Function),
                &[0.99, 0.01],
            ),
            with_embedding(
                symbol("sym-c", "gamma", "crate::gamma", SymbolKind::Function),
                &[0.98, 0.02],
            ),
            with_embedding(
                symbol("sym-d", "delta", "crate::delta", SymbolKind::Function),
                &[0.97, 0.03],
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(1, 2, 1);
        graph.add_edge(2, 3, 1);
        graph.add_edge(3, 4, 1);
        graph.add_edge(2, 4, 1);

        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config,
        );

        assert!(rescued >= 1);
        assert!(graph.has_edge(0, 2));
        assert!(graph.has_edge(0, 3));
        assert!(!graph.has_edge(0, 4));
        assert_eq!(graph.neighbors(0).len(), 3);
    }

    #[test]
    fn semantic_rescue_does_not_bridge_components() {
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
                &[0.8, 0.6],
            ),
            symbol("sym-b2", "b2", "crate::b2", SymbolKind::Function),
            with_embedding(
                symbol(
                    "sym-orphan",
                    "orphan",
                    "crate::orphan",
                    SymbolKind::Function,
                ),
                &[0.95, 0.2],
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let rep_to_members = (0..entries.len())
            .map(|index| vec![index])
            .collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(1, 2, 1);
        graph.add_edge(3, 4, 1);
        graph.add_edge(4, 5, 1);

        let (before_components, _) =
            count_components_and_largest(&graph, rep_to_members.as_slice(), entries.as_slice());
        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );
        let (after_components, _) =
            count_components_and_largest(&graph, rep_to_members.as_slice(), entries.as_slice());

        assert_eq!(before_components, 3);
        assert_eq!(rescued, 1);
        assert_eq!(after_components, 2);
        assert_eq!(graph.degree(6), 1);
        assert!(graph.has_edge(6, 1));
        assert!(!graph.has_edge(6, 4));
        assert!(!graph.has_edge(1, 4));
    }

    #[test]
    fn semantic_rescue_densifies_within_component() {
        let symbols = vec![
            with_embedding(
                symbol("sym-a0", "a0", "crate::a0", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            symbol("sym-a1", "a1", "crate::a1", SymbolKind::Function),
            with_embedding(
                symbol("sym-a2", "a2", "crate::a2", SymbolKind::Function),
                &[0.99, 0.01],
            ),
            symbol("sym-a3", "a3", "crate::a3", SymbolKind::Function),
            symbol("sym-a4", "a4", "crate::a4", SymbolKind::Function),
            with_embedding(
                symbol("sym-b0", "b0", "crate::b0", SymbolKind::Function),
                &[0.999, 0.001],
            ),
            symbol("sym-b1", "b1", "crate::b1", SymbolKind::Function),
            symbol("sym-b2", "b2", "crate::b2", SymbolKind::Function),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let rep_to_members = (0..entries.len())
            .map(|index| vec![index])
            .collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(1, 2, 1);
        graph.add_edge(2, 3, 1);
        graph.add_edge(3, 4, 1);
        graph.add_edge(2, 4, 1);
        graph.add_edge(5, 6, 1);
        graph.add_edge(6, 7, 1);

        let (before_components, _) =
            count_components_and_largest(&graph, rep_to_members.as_slice(), entries.as_slice());
        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );
        let (after_components, _) =
            count_components_and_largest(&graph, rep_to_members.as_slice(), entries.as_slice());

        assert!(rescued >= 1);
        assert_eq!(before_components, after_components);
        assert!(graph.has_edge(0, 2));
        assert!(!graph.has_edge(0, 5));
    }

    #[test]
    fn semantic_rescue_orphan_becomes_leaf() {
        let symbols = vec![
            symbol("sym-a0", "a0", "crate::a0", SymbolKind::Function),
            with_embedding(
                symbol("sym-a1", "a1", "crate::a1", SymbolKind::Function),
                &[1.0, 0.0],
            ),
            symbol("sym-a2", "a2", "crate::a2", SymbolKind::Function),
            with_embedding(
                symbol(
                    "sym-orphan",
                    "orphan",
                    "crate::orphan",
                    SymbolKind::Function,
                ),
                &[0.99, 0.01],
            ),
        ];
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let rep_to_members = (0..entries.len())
            .map(|index| vec![index])
            .collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(1, 2, 1);

        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );
        let (components, _) =
            count_components_and_largest(&graph, rep_to_members.as_slice(), entries.as_slice());

        assert_eq!(rescued, 1);
        assert_eq!(components, 1);
        assert_eq!(graph.degree(3), 1);
        assert_eq!(graph.neighbors(3), vec![(1, 1)]);
    }

    #[test]
    fn semantic_rescue_orphan_prefers_existing_component() {
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
                &[0.82, 0.58],
            ),
            symbol("sym-b2", "b2", "crate::b2", SymbolKind::Function),
            with_embedding(
                symbol(
                    "sym-singleton",
                    "singleton",
                    "crate::singleton",
                    SymbolKind::Function,
                ),
                &[0.999, 0.001],
            ),
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
        let entries = entries(&symbols);
        let rep_by_index = (0..entries.len()).collect::<Vec<_>>();
        let mut graph = WeightedGraph::default();
        graph.add_edge(0, 1, 1);
        graph.add_edge(1, 2, 1);
        graph.add_edge(3, 4, 1);
        graph.add_edge(4, 5, 1);

        let rescued = apply_semantic_rescue(
            entries.as_slice(),
            rep_by_index.as_slice(),
            &mut graph,
            &config(),
        );

        assert!(rescued >= 1);
        assert_eq!(graph.degree(7), 1);
        assert!(graph.has_edge(7, 1));
        assert!(!graph.has_edge(7, 6));
    }
}
