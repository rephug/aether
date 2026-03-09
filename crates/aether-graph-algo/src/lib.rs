use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::Direction;
use petgraph::algo::kosaraju_scc;
use petgraph::graph::{DiGraph, NodeIndex, UnGraph};
use petgraph::unionfind::UnionFind;
use petgraph::visit::{Bfs, EdgeRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphAlgorithmEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_kind: String,
}

pub fn build_digraph(
    edges: &[GraphAlgorithmEdge],
) -> (
    DiGraph<String, String>,
    HashMap<String, NodeIndex>,
    HashMap<NodeIndex, String>,
) {
    let mut graph = DiGraph::<String, String>::new();
    let mut by_id = HashMap::<String, NodeIndex>::new();

    for edge in edges {
        for node_id in [&edge.source_id, &edge.target_id] {
            if by_id.contains_key(node_id.as_str()) {
                continue;
            }
            let idx = graph.add_node(node_id.clone());
            by_id.insert(node_id.clone(), idx);
        }
        let source = by_id[edge.source_id.as_str()];
        let target = by_id[edge.target_id.as_str()];
        graph.add_edge(source, target, edge.edge_kind.clone());
    }

    let names = by_id
        .iter()
        .map(|(id, idx)| (*idx, id.clone()))
        .collect::<HashMap<_, _>>();
    (graph, by_id, names)
}

pub fn build_undirected_weighted_graph(
    edges: &[GraphAlgorithmEdge],
) -> (
    UnGraph<String, f64>,
    HashMap<String, NodeIndex>,
    HashMap<NodeIndex, String>,
) {
    let mut graph = UnGraph::<String, f64>::new_undirected();
    let mut by_id = HashMap::<String, NodeIndex>::new();
    let mut weights = HashMap::<(NodeIndex, NodeIndex), f64>::new();

    for edge in edges {
        for node_id in [&edge.source_id, &edge.target_id] {
            if by_id.contains_key(node_id.as_str()) {
                continue;
            }
            let idx = graph.add_node(node_id.clone());
            by_id.insert(node_id.clone(), idx);
        }
        let a = by_id[edge.source_id.as_str()];
        let b = by_id[edge.target_id.as_str()];
        let (lhs, rhs) = if a.index() <= b.index() {
            (a, b)
        } else {
            (b, a)
        };
        *weights.entry((lhs, rhs)).or_insert(0.0) += 1.0;
    }

    for ((a, b), weight) in weights {
        graph.add_edge(a, b, weight);
    }

    let names = by_id
        .iter()
        .map(|(id, idx)| (*idx, id.clone()))
        .collect::<HashMap<_, _>>();
    (graph, by_id, names)
}

pub fn bfs_shortest_path_sync(
    edges: &[GraphAlgorithmEdge],
    from_id: &str,
    to_id: &str,
) -> Option<Vec<String>> {
    let (graph, by_id, names) = build_digraph(edges);
    let &start = by_id.get(from_id)?;
    let &goal = by_id.get(to_id)?;
    if start == goal {
        return Some(vec![from_id.to_owned()]);
    }

    let mut bfs = Bfs::new(&graph, start);
    let mut visited = HashSet::from([start]);
    let mut parent = HashMap::<NodeIndex, NodeIndex>::new();

    while let Some(node) = bfs.next(&graph) {
        for next in graph.neighbors_directed(node, Direction::Outgoing) {
            if !visited.insert(next) {
                continue;
            }
            parent.insert(next, node);
            if next == goal {
                let mut path = vec![goal];
                let mut cur = goal;
                while let Some(&p) = parent.get(&cur) {
                    path.push(p);
                    if p == start {
                        break;
                    }
                    cur = p;
                }
                path.reverse();
                let materialized = path
                    .into_iter()
                    .filter_map(|idx| names.get(&idx).cloned())
                    .collect::<Vec<_>>();
                return Some(materialized);
            }
        }
    }

    None
}

pub fn page_rank_sync(
    edges: &[GraphAlgorithmEdge],
    damping: f64,
    iterations: usize,
) -> Vec<(String, f64)> {
    let (graph, _, names) = build_digraph(edges);
    let node_count = graph.node_count();
    if node_count == 0 {
        return Vec::new();
    }

    let nodes = graph.node_indices().collect::<Vec<_>>();
    let node_count_f = node_count as f64;
    let mut rank = HashMap::<NodeIndex, f64>::new();
    for node in &nodes {
        rank.insert(*node, 1.0 / node_count_f);
    }
    let base = (1.0 - damping) / node_count_f;

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

        let mut next = HashMap::<NodeIndex, f64>::new();
        for &node in &nodes {
            let incoming_sum = graph
                .neighbors_directed(node, Direction::Incoming)
                .map(|inbound| {
                    let degree = graph
                        .neighbors_directed(inbound, Direction::Outgoing)
                        .count()
                        .max(1);
                    rank.get(&inbound).copied().unwrap_or(0.0) / degree as f64
                })
                .sum::<f64>();

            next.insert(
                node,
                base + damping * (incoming_sum + (dangling_sum / node_count_f)),
            );
        }
        rank = next;
    }

    let mut scored = rank
        .into_iter()
        .filter_map(|(node, score)| names.get(&node).cloned().map(|name| (name, score)))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored
}

pub fn betweenness_centrality_sync(edges: &[GraphAlgorithmEdge]) -> Vec<(String, f64)> {
    let (graph, _, names) = build_digraph(edges);
    let nodes = graph.node_indices().collect::<Vec<_>>();
    let node_count = nodes.len();
    if node_count == 0 {
        return Vec::new();
    }

    let mut centrality = nodes
        .iter()
        .copied()
        .map(|node| (node, 0.0f64))
        .collect::<HashMap<_, _>>();

    for &source in &nodes {
        let mut stack = Vec::<NodeIndex>::new();
        let mut predecessors = nodes
            .iter()
            .copied()
            .map(|node| (node, Vec::<NodeIndex>::new()))
            .collect::<HashMap<_, _>>();
        let mut sigma = nodes
            .iter()
            .copied()
            .map(|node| (node, 0.0f64))
            .collect::<HashMap<_, _>>();
        let mut distance = nodes
            .iter()
            .copied()
            .map(|node| (node, -1i64))
            .collect::<HashMap<_, _>>();

        sigma.insert(source, 1.0);
        distance.insert(source, 0);

        let mut queue = VecDeque::new();
        queue.push_back(source);

        while let Some(node) = queue.pop_front() {
            stack.push(node);
            let node_distance = *distance.get(&node).unwrap_or(&-1);

            for neighbor in graph.neighbors_directed(node, Direction::Outgoing) {
                if *distance.get(&neighbor).unwrap_or(&-1) < 0 {
                    distance.insert(neighbor, node_distance + 1);
                    queue.push_back(neighbor);
                }

                if *distance.get(&neighbor).unwrap_or(&-1) == node_distance + 1 {
                    predecessors.entry(neighbor).or_default().push(node);
                    let sigma_node = *sigma.get(&node).unwrap_or(&0.0);
                    *sigma.entry(neighbor).or_insert(0.0) += sigma_node;
                }
            }
        }

        let mut dependency = nodes
            .iter()
            .copied()
            .map(|node| (node, 0.0f64))
            .collect::<HashMap<_, _>>();

        while let Some(node) = stack.pop() {
            let sigma_node = *sigma.get(&node).unwrap_or(&0.0);
            if sigma_node <= f64::EPSILON {
                continue;
            }

            let pred = predecessors.remove(&node).unwrap_or_default();
            for predecessor in pred {
                let sigma_predecessor = *sigma.get(&predecessor).unwrap_or(&0.0);
                if sigma_predecessor <= f64::EPSILON {
                    continue;
                }
                let contribution = (sigma_predecessor / sigma_node)
                    * (1.0 + dependency.get(&node).copied().unwrap_or(0.0));
                *dependency.entry(predecessor).or_insert(0.0) += contribution;
            }

            if node != source {
                *centrality.entry(node).or_insert(0.0) +=
                    dependency.get(&node).copied().unwrap_or(0.0);
            }
        }
    }

    let normalization = if node_count > 2 {
        ((node_count - 1) * (node_count - 2)) as f64
    } else {
        0.0
    };

    let mut scored = nodes
        .into_iter()
        .filter_map(|node| {
            let name = names.get(&node)?.clone();
            let raw = centrality.get(&node).copied().unwrap_or(0.0);
            let score = if normalization > f64::EPSILON {
                raw / normalization
            } else {
                0.0
            };
            Some((name, score))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored
}

pub fn louvain_sync(edges: &[GraphAlgorithmEdge]) -> Vec<(String, usize)> {
    louvain_with_resolution_sync(edges, 1.0)
}

pub fn louvain_with_resolution_sync(
    edges: &[GraphAlgorithmEdge],
    resolution: f64,
) -> Vec<(String, usize)> {
    let (graph, _, names) = build_undirected_weighted_graph(edges);
    if graph.node_count() == 0 {
        return Vec::new();
    }

    let total_weight = graph.edge_weights().copied().sum::<f64>();
    if total_weight <= f64::EPSILON {
        let mut singleton = names
            .into_values()
            .enumerate()
            .map(|(idx, node)| (node, idx + 1))
            .collect::<Vec<_>>();
        singleton.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        return singleton;
    }

    let mut node_order = graph.node_indices().collect::<Vec<_>>();
    node_order.sort_by(|left, right| {
        names[left]
            .cmp(&names[right])
            .then_with(|| left.index().cmp(&right.index()))
    });

    let mut communities = HashMap::<NodeIndex, usize>::new();
    let mut degrees = HashMap::<NodeIndex, f64>::new();
    let mut sum_tot = HashMap::<usize, f64>::new();

    for node in graph.node_indices() {
        let degree = graph.edges(node).map(|edge| *edge.weight()).sum::<f64>();
        degrees.insert(node, degree);
        let community = node.index();
        communities.insert(node, community);
        sum_tot.insert(community, degree);
    }

    let two_m = 2.0 * total_weight;
    for _ in 0..50 {
        let mut moved_any = false;

        for &node in &node_order {
            let current = communities[&node];
            let k_i = *degrees.get(&node).unwrap_or(&0.0);
            if k_i <= f64::EPSILON {
                continue;
            }

            let mut neighbor_weight_by_community = HashMap::<usize, f64>::new();
            for edge in graph.edges(node) {
                let neighbor = if edge.source() == node {
                    edge.target()
                } else {
                    edge.source()
                };
                let community = communities[&neighbor];
                *neighbor_weight_by_community.entry(community).or_insert(0.0) += *edge.weight();
            }

            if let Some(total) = sum_tot.get_mut(&current) {
                *total -= k_i;
            }

            let mut best_community = current;
            let mut best_gain = 0.0f64;
            let mut candidates = neighbor_weight_by_community
                .keys()
                .copied()
                .collect::<Vec<_>>();
            candidates.push(current);
            candidates.sort_unstable();
            candidates.dedup();

            for candidate in candidates {
                let k_i_in = *neighbor_weight_by_community.get(&candidate).unwrap_or(&0.0);
                let sum_tot_candidate = *sum_tot.get(&candidate).unwrap_or(&0.0);
                let gain = k_i_in - resolution.max(0.0) * (k_i * sum_tot_candidate) / two_m;
                if gain > best_gain + 1e-12
                    || ((gain - best_gain).abs() <= 1e-12 && candidate < best_community)
                {
                    best_gain = gain;
                    best_community = candidate;
                }
            }

            communities.insert(node, best_community);
            *sum_tot.entry(best_community).or_insert(0.0) += k_i;
            if best_community != current {
                moved_any = true;
            }
        }

        if !moved_any {
            break;
        }
    }

    let mut members_by_community = HashMap::<usize, Vec<String>>::new();
    for (node, community) in &communities {
        members_by_community
            .entry(*community)
            .or_default()
            .push(names[node].clone());
    }
    for members in members_by_community.values_mut() {
        members.sort();
    }

    let mut community_order = members_by_community
        .iter()
        .map(|(community, members)| (*community, members.clone()))
        .collect::<Vec<_>>();
    community_order.sort_by(|left, right| {
        left.1
            .len()
            .cmp(&right.1.len())
            .then_with(|| left.1.join(",").cmp(&right.1.join(",")))
    });

    let remapped = community_order
        .into_iter()
        .enumerate()
        .map(|(idx, (old, _))| (old, idx + 1))
        .collect::<HashMap<_, _>>();

    let mut assignments = communities
        .into_iter()
        .filter_map(|(node, community)| {
            let node_id = names.get(&node)?.clone();
            let community_id = *remapped.get(&community)?;
            Some((node_id, community_id))
        })
        .collect::<Vec<_>>();
    assignments.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    assignments
}

pub fn strongly_connected_components_sync(edges: &[GraphAlgorithmEdge]) -> Vec<Vec<String>> {
    let (graph, _, names) = build_digraph(edges);
    let mut components = kosaraju_scc(&graph)
        .into_iter()
        .map(|component| {
            let mut nodes = component
                .into_iter()
                .filter_map(|idx| names.get(&idx).cloned())
                .collect::<Vec<_>>();
            nodes.sort();
            nodes
        })
        .collect::<Vec<_>>();
    components.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.join(",").cmp(&right.join(",")))
    });
    components
}

pub fn connected_components_sync(edges: &[GraphAlgorithmEdge]) -> Vec<Vec<String>> {
    let (graph, _, names) = build_digraph(edges);
    let mut union_find = UnionFind::new(graph.node_count());

    for edge in graph.edge_references() {
        union_find.union(edge.source().index(), edge.target().index());
    }

    let mut components_by_root = HashMap::<usize, Vec<String>>::new();
    for node in graph.node_indices() {
        let root = union_find.find(node.index());
        if let Some(name) = names.get(&node) {
            components_by_root
                .entry(root)
                .or_default()
                .push(name.clone());
        }
    }

    let mut components = components_by_root.into_values().collect::<Vec<_>>();
    for component in &mut components {
        component.sort();
        component.dedup();
    }

    components.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.join(",").cmp(&right.join(",")))
    });
    components
}

pub fn cross_community_edges_sync(
    edges: &[GraphAlgorithmEdge],
    communities: &HashMap<String, usize>,
) -> Vec<(String, String, String)> {
    let mut out = edges
        .iter()
        .filter(|edge| {
            let Some(src) = communities.get(edge.source_id.as_str()) else {
                return false;
            };
            let Some(dst) = communities.get(edge.target_id.as_str()) else {
                return false;
            };
            src != dst
        })
        .map(|edge| {
            (
                edge.source_id.clone(),
                edge.target_id.clone(),
                edge.edge_kind.clone(),
            )
        })
        .collect::<Vec<_>>();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(source: &str, target: &str) -> GraphAlgorithmEdge {
        GraphAlgorithmEdge {
            source_id: source.to_owned(),
            target_id: target.to_owned(),
            edge_kind: "calls".to_owned(),
        }
    }

    #[test]
    fn bfs_shortest_path_finds_expected_route() {
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("a", "d"),
            edge("d", "e"),
        ];
        let path = bfs_shortest_path_sync(&edges, "a", "c").expect("path");
        assert_eq!(path, vec!["a", "b", "c"]);
    }

    #[test]
    fn pagerank_sorts_higher_central_node_first() {
        let edges = vec![edge("a", "b"), edge("c", "b"), edge("b", "d")];
        let scores = page_rank_sync(&edges, 0.85, 25);
        assert!(!scores.is_empty());
        let by_node = scores.into_iter().collect::<HashMap<_, _>>();
        assert!(by_node["b"] > by_node["a"]);
        assert!(by_node["b"] > by_node["c"]);
    }

    #[test]
    fn components_and_scc_are_sorted_by_size() {
        let edges = vec![
            edge("a", "b"),
            edge("b", "a"),
            edge("c", "d"),
            edge("d", "e"),
        ];
        let scc = strongly_connected_components_sync(&edges);
        assert!(
            scc.iter()
                .any(|component| component == &vec!["a".to_owned(), "b".to_owned()])
        );

        let cc = connected_components_sync(&edges);
        assert_eq!(cc.len(), 2);
        assert!(cc[0].len() >= cc[1].len());
    }

    #[test]
    fn louvain_groups_disconnected_clusters() {
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("x", "y"),
            edge("y", "z"),
        ];
        let assignments = louvain_sync(&edges);
        let community_count = assignments
            .iter()
            .map(|(_, community)| *community)
            .collect::<HashSet<_>>()
            .len();
        assert_eq!(community_count, 2);
    }

    #[test]
    fn louvain_with_resolution_one_matches_standard_louvain() {
        let edges = vec![
            edge("a", "b"),
            edge("a", "c"),
            edge("b", "c"),
            edge("c", "d"),
            edge("d", "e"),
            edge("d", "f"),
            edge("e", "f"),
        ];

        assert_eq!(
            louvain_sync(&edges),
            louvain_with_resolution_sync(&edges, 1.0)
        );
    }

    #[test]
    fn louvain_with_resolution_produces_fewer_communities_at_low_gamma() {
        let edges = vec![
            edge("a", "b"),
            edge("a", "c"),
            edge("a", "d"),
            edge("b", "c"),
        ];

        let low = louvain_with_resolution_sync(&edges, 0.4);
        let standard = louvain_sync(&edges);
        let low_count = low
            .iter()
            .map(|(_, community)| *community)
            .collect::<HashSet<_>>()
            .len();
        let standard_count = standard
            .iter()
            .map(|(_, community)| *community)
            .collect::<HashSet<_>>()
            .len();

        assert!(low_count < standard_count);
    }

    #[test]
    fn louvain_with_resolution_high_gamma_more_communities() {
        let edges = vec![
            edge("a", "b"),
            edge("a", "c"),
            edge("a", "d"),
            edge("a", "e"),
        ];

        let standard = louvain_sync(&edges);
        let high = louvain_with_resolution_sync(&edges, 1.8);
        let standard_count = standard
            .iter()
            .map(|(_, community)| *community)
            .collect::<HashSet<_>>()
            .len();
        let high_count = high
            .iter()
            .map(|(_, community)| *community)
            .collect::<HashSet<_>>()
            .len();

        assert!(high_count > standard_count);
    }

    #[test]
    fn cross_community_edges_filters_intra_community_edges() {
        let edges = vec![edge("a", "b"), edge("b", "c"), edge("c", "d")];
        let communities = HashMap::from([
            ("a".to_owned(), 1usize),
            ("b".to_owned(), 1usize),
            ("c".to_owned(), 2usize),
            ("d".to_owned(), 2usize),
        ]);
        let cross = cross_community_edges_sync(&edges, &communities);
        assert_eq!(
            cross,
            vec![("b".to_owned(), "c".to_owned(), "calls".to_owned())]
        );
    }

    #[test]
    fn betweenness_center_of_star_is_highest() {
        let edges = vec![
            edge("a", "center"),
            edge("b", "center"),
            edge("d", "center"),
            edge("center", "a"),
            edge("center", "b"),
            edge("center", "d"),
        ];
        let scores = betweenness_centrality_sync(&edges)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert!(scores["center"] > scores["a"]);
        assert!(scores["center"] > scores["b"]);
        assert!(scores["center"] > scores["d"]);
    }

    #[test]
    fn betweenness_chain_middle_nodes_are_higher() {
        let edges = vec![edge("a", "b"), edge("b", "c"), edge("c", "d")];
        let scores = betweenness_centrality_sync(&edges)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert!(scores["b"] > scores["a"]);
        assert!(scores["c"] > scores["d"]);
        assert!(scores["b"] > scores["d"]);
    }

    #[test]
    fn betweenness_disconnected_graph_is_zero() {
        let edges = vec![edge("a", "b"), edge("c", "d")];
        let scores = betweenness_centrality_sync(&edges)
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert_eq!(scores["a"], 0.0);
        assert_eq!(scores["b"], 0.0);
        assert_eq!(scores["c"], 0.0);
        assert_eq!(scores["d"], 0.0);
    }

    #[test]
    fn betweenness_empty_graph_is_empty() {
        let scores = betweenness_centrality_sync(&[]);
        assert!(scores.is_empty());
    }
}
