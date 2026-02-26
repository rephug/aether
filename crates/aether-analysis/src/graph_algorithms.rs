use std::collections::HashMap;

pub use aether_graph_algo::GraphAlgorithmEdge;

pub fn bfs_shortest_path(edges: &[GraphAlgorithmEdge], from_id: &str, to_id: &str) -> Vec<String> {
    aether_graph_algo::bfs_shortest_path_sync(edges, from_id, to_id).unwrap_or_default()
}

pub fn page_rank(
    edges: &[GraphAlgorithmEdge],
    damping: f64,
    iterations: usize,
) -> HashMap<String, f64> {
    aether_graph_algo::page_rank_sync(edges, damping, iterations)
        .into_iter()
        .collect()
}

pub fn louvain_communities(edges: &[GraphAlgorithmEdge]) -> HashMap<String, usize> {
    aether_graph_algo::louvain_sync(edges).into_iter().collect()
}

pub fn strongly_connected_components(edges: &[GraphAlgorithmEdge]) -> Vec<Vec<String>> {
    aether_graph_algo::strongly_connected_components_sync(edges)
}

pub fn connected_components(edges: &[GraphAlgorithmEdge]) -> Vec<Vec<String>> {
    aether_graph_algo::connected_components_sync(edges)
}

pub fn cross_community_edges(
    edges: &[GraphAlgorithmEdge],
    communities: &HashMap<String, usize>,
) -> Vec<(String, String, String)> {
    aether_graph_algo::cross_community_edges_sync(edges, communities)
}
