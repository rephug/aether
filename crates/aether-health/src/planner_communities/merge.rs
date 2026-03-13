use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::SymbolEntry;
use super::graph::WeightedGraph;
use super::rescue::cosine_similarity;

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

pub(super) fn merge_small_communities(
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
                        structural_edges: structural_graph.structural_edges_between(
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

pub(super) fn finalize_assignments(
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

fn compare_optional_f32(left: Option<f32>, right: Option<f32>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aether_core::SymbolKind;

    use super::super::graph::WeightedGraph;
    use super::super::{FileSymbol, SymbolEntry, qualified_name_stem};
    use super::merge_small_communities;

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
}
