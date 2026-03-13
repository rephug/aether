use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use aether_graph_algo::GraphAlgorithmEdge;

use super::{DisjointSet, SymbolEntry};

#[derive(Clone, Default)]
pub(super) struct WeightedGraph {
    edges: BTreeMap<(usize, usize), usize>,
}

impl WeightedGraph {
    pub(super) fn add_edge(&mut self, left: usize, right: usize, weight: usize) {
        if left == right || weight == 0 {
            return;
        }
        let key = normalized_pair(left, right);
        *self.edges.entry(key).or_default() += weight;
    }

    pub(super) fn has_edge(&self, left: usize, right: usize) -> bool {
        if left == right {
            return false;
        }
        self.edges.contains_key(&normalized_pair(left, right))
    }

    pub(super) fn degree(&self, node: usize) -> usize {
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

    pub(super) fn neighbors(&self, node: usize) -> Vec<(usize, usize)> {
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

    pub(super) fn connected_components(
        &self,
        nodes: &[usize],
        entries: &[SymbolEntry],
    ) -> Vec<Vec<usize>> {
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

    pub(super) fn repeated_component_edges(&self, component: &[usize]) -> Vec<GraphAlgorithmEdge> {
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

    pub(super) fn structural_edges_between(
        &self,
        rep_to_community: &HashMap<usize, usize>,
        left_community: usize,
        right_community: usize,
    ) -> usize {
        self.edges
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
}

pub(super) fn build_rep_members(len: usize, union_find: &mut DisjointSet) -> Vec<Vec<usize>> {
    let mut reps = vec![Vec::new(); len];
    for index in 0..len {
        let rep = union_find.find(index);
        if let Some(slot) = reps.get_mut(rep) {
            slot.push(index);
        }
    }
    reps
}

pub(super) fn collapse_structural_edges(
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

pub(super) fn normalized_pair(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}
