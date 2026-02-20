use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

use super::{
    CouplingEdgeRecord, GraphStore, ResolvedEdge, StoreError, SymbolRecord, TestedByRecord,
    UpstreamDependencyEdgeRecord, UpstreamDependencyNodeRecord, UpstreamDependencyTraversal,
};

pub struct CozoGraphStore {
    db: DbInstance,
}

pub type CrossCommunityEdge = (String, String, String, i64, i64);

impl CozoGraphStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let aether_dir = workspace_root.join(".aether");
        let graph_path = aether_dir.join("graph.db");
        fs::create_dir_all(&aether_dir)?;

        let graph_path_str = graph_path.to_string_lossy().to_string();
        let db = DbInstance::new("sled", &graph_path_str, Default::default())
            .map_err(|err| StoreError::Cozo(err.to_string()))?;
        let store = Self { db };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        self.ensure_relation(
            r#"
            :create symbols {
                symbol_id: String =>
                qualified_name: String,
                name: String,
                kind: String,
                file_path: String,
                language: String,
                signature_fingerprint: String,
                last_seen_at: Int
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create edges {
                source_id: String,
                target_id: String,
                edge_kind: String =>
                file_path: String
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create co_change_edges {
                file_a: String,
                file_b: String =>
                co_change_count: Int,
                total_commits_a: Int,
                total_commits_b: Int,
                git_coupling: Float,
                static_signal: Float,
                semantic_signal: Float,
                fused_score: Float,
                coupling_type: String,
                last_co_change_commit: String,
                last_co_change_at: Int,
                mined_at: Int
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create tested_by {
                target_file: String,
                test_file: String =>
                intent_count: Int,
                confidence: Float,
                inference_method: String
            }
            "#,
        )?;
        Ok(())
    }

    fn ensure_relation(&self, script: &str) -> Result<(), StoreError> {
        match self.run_script(script, BTreeMap::new(), ScriptMutability::Mutable) {
            Ok(_) => Ok(()),
            Err(StoreError::Cozo(message))
                if message.contains("relation exists")
                    || message.contains("already exists")
                    || message.contains("conflicts with an existing one")
                    || message.contains("Duplicated")
                    || message.contains("duplicate") =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn run_script(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
        mutability: ScriptMutability,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(script, params, mutability)
            .map_err(|err| StoreError::Cozo(err.to_string()))
    }

    fn row_to_symbol(row: &[DataValue]) -> Result<SymbolRecord, StoreError> {
        if row.len() < 7 {
            return Err(StoreError::Cozo("invalid symbol row shape".to_owned()));
        }

        let last_seen_at = row[6]
            .get_int()
            .ok_or_else(|| StoreError::Cozo("invalid last_seen_at value".to_owned()))?;

        Ok(SymbolRecord {
            id: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid symbol id value".to_owned()))?
                .to_owned(),
            file_path: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_path value".to_owned()))?
                .to_owned(),
            language: row[2]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid language value".to_owned()))?
                .to_owned(),
            kind: row[3]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid kind value".to_owned()))?
                .to_owned(),
            qualified_name: row[4]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid qualified_name value".to_owned()))?
                .to_owned(),
            signature_fingerprint: row[5]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid signature_fingerprint value".to_owned()))?
                .to_owned(),
            last_seen_at,
        })
    }

    fn symbol_name(qualified_name: &str) -> &str {
        qualified_name
            .rsplit("::")
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(qualified_name)
    }

    fn row_to_coupling_edge(row: &[DataValue]) -> Result<CouplingEdgeRecord, StoreError> {
        if row.len() < 13 {
            return Err(StoreError::Cozo(
                "invalid co_change_edges row shape".to_owned(),
            ));
        }

        let as_f32 = |index: usize, label: &str| -> Result<f32, StoreError> {
            let value = &row[index];
            if let Some(value) = value.get_float() {
                return Ok(value as f32);
            }
            if let Some(value) = value.get_int() {
                return Ok(value as f32);
            }
            Err(StoreError::Cozo(format!("invalid {label} value")))
        };

        Ok(CouplingEdgeRecord {
            file_a: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_a value".to_owned()))?
                .to_owned(),
            file_b: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_b value".to_owned()))?
                .to_owned(),
            co_change_count: row[2]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid co_change_count value".to_owned()))?,
            total_commits_a: row[3]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid total_commits_a value".to_owned()))?,
            total_commits_b: row[4]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid total_commits_b value".to_owned()))?,
            git_coupling: as_f32(5, "git_coupling")?,
            static_signal: as_f32(6, "static_signal")?,
            semantic_signal: as_f32(7, "semantic_signal")?,
            fused_score: as_f32(8, "fused_score")?,
            coupling_type: row[9]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid coupling_type value".to_owned()))?
                .to_owned(),
            last_co_change_commit: row[10]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid last_co_change_commit value".to_owned()))?
                .to_owned(),
            last_co_change_at: row[11]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid last_co_change_at value".to_owned()))?,
            mined_at: row[12]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid mined_at value".to_owned()))?,
        })
    }

    fn row_to_tested_by(row: &[DataValue]) -> Result<TestedByRecord, StoreError> {
        if row.len() < 5 {
            return Err(StoreError::Cozo("invalid tested_by row shape".to_owned()));
        }

        let confidence = if let Some(value) = row[3].get_float() {
            value as f32
        } else if let Some(value) = row[3].get_int() {
            value as f32
        } else {
            return Err(StoreError::Cozo("invalid confidence value".to_owned()));
        };

        Ok(TestedByRecord {
            target_file: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid target_file value".to_owned()))?
                .to_owned(),
            test_file: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid test_file value".to_owned()))?
                .to_owned(),
            intent_count: row[2]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid intent_count value".to_owned()))?,
            confidence,
            inference_method: row[4]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid inference_method value".to_owned()))?
                .to_owned(),
        })
    }

    fn list_dependency_edges_raw(&self) -> Result<Vec<(String, String, String)>, StoreError> {
        let rows = self.run_script(
            r#"
            ?[source_id, target_id, edge_kind] :=
                *edges{source_id, target_id, edge_kind, file_path}
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut edges = Vec::new();
        for row in rows.rows {
            if row.len() < 3 {
                return Err(StoreError::Cozo("invalid edge row shape".to_owned()));
            }
            let source_id = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid edge source_id value".to_owned()))?
                .to_owned();
            let target_id = row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid edge target_id value".to_owned()))?
                .to_owned();
            let edge_kind = row[2]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid edge_kind value".to_owned()))?
                .to_owned();

            if !matches!(edge_kind.as_str(), "calls" | "depends_on") {
                continue;
            }

            edges.push((source_id, target_id, edge_kind));
        }
        Ok(edges)
    }

    pub fn list_dependency_edges(&self) -> Result<Vec<(String, String, String)>, StoreError> {
        self.list_dependency_edges_raw()
    }

    fn try_louvain_from_cozo(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "calls"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "depends_on"

            ?[node, community] := community_detection_louvain(*dep_edges[], node, community)
            :order node
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut assignments = Vec::new();
        for row in rows.rows {
            if row.len() < 2 {
                return Err(StoreError::Cozo("invalid louvain row shape".to_owned()));
            }
            let node = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid louvain node value".to_owned()))?
                .to_owned();
            let community = data_value_to_i64(&row[1])
                .ok_or_else(|| StoreError::Cozo("invalid louvain community_id value".to_owned()))?;
            assignments.push((node, community));
        }
        Ok(assignments)
    }

    fn try_pagerank_from_cozo(&self) -> Result<Vec<(String, f32)>, StoreError> {
        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "calls"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "depends_on"

            ?[node, rank] := pagerank(*dep_edges[], node, rank)
            :order node
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut scores = Vec::new();
        for row in rows.rows {
            if row.len() < 2 {
                return Err(StoreError::Cozo("invalid pagerank row shape".to_owned()));
            }
            let node = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid pagerank node value".to_owned()))?
                .to_owned();
            let rank = if let Some(value) = row[1].get_float() {
                value as f32
            } else if let Some(value) = row[1].get_int() {
                value as f32
            } else {
                return Err(StoreError::Cozo("invalid pagerank value".to_owned()));
            };
            scores.push((node, rank));
        }
        Ok(scores)
    }

    fn try_betweenness_from_cozo(&self) -> Result<Vec<(String, f32)>, StoreError> {
        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "calls"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "depends_on"

            ?[node, centrality] := betweenness_centrality(*dep_edges[], node, centrality)
            :order node
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut scores = Vec::new();
        for row in rows.rows {
            if row.len() < 2 {
                return Err(StoreError::Cozo("invalid betweenness row shape".to_owned()));
            }
            let node = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid betweenness node value".to_owned()))?
                .to_owned();
            let centrality = if let Some(value) = row[1].get_float() {
                value as f32
            } else if let Some(value) = row[1].get_int() {
                value as f32
            } else {
                return Err(StoreError::Cozo("invalid betweenness value".to_owned()));
            };
            scores.push((node, centrality));
        }
        Ok(scores)
    }

    fn try_scc_from_cozo(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "calls"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "depends_on"

            ?[node, component] := strongly_connected_components(*dep_edges[], node, component)
            :order component, node
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut grouped = BTreeMap::<i64, Vec<String>>::new();
        for row in rows.rows {
            if row.len() < 2 {
                return Err(StoreError::Cozo("invalid scc row shape".to_owned()));
            }
            let node = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid scc node value".to_owned()))?
                .to_owned();
            let component = data_value_to_i64(&row[1])
                .ok_or_else(|| StoreError::Cozo("invalid scc component value".to_owned()))?;
            grouped.entry(component).or_default().push(node);
        }
        Ok(grouped.into_values().collect())
    }

    fn try_connected_components_from_cozo(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "calls"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind},
                edge_kind = "depends_on"

            ?[node, component] := connected_components(*dep_edges[], node, component)
            :order component, node
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut grouped = BTreeMap::<i64, Vec<String>>::new();
        for row in rows.rows {
            if row.len() < 2 {
                return Err(StoreError::Cozo(
                    "invalid connected components row shape".to_owned(),
                ));
            }
            let node = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid component node value".to_owned()))?
                .to_owned();
            let component = data_value_to_i64(&row[1])
                .ok_or_else(|| StoreError::Cozo("invalid connected component value".to_owned()))?;
            grouped.entry(component).or_default().push(node);
        }
        Ok(grouped.into_values().collect())
    }

    pub fn list_louvain_communities(&self) -> Result<Vec<(String, i64)>, StoreError> {
        match self.try_louvain_from_cozo() {
            Ok(mut records) => {
                records.sort_by(|left, right| left.0.cmp(&right.0));
                Ok(records)
            }
            Err(_) => {
                let components = self.list_connected_components_fallback()?;
                let mut assignments = Vec::new();
                for (index, component) in components.into_iter().enumerate() {
                    for node in component {
                        assignments.push((node, index as i64 + 1));
                    }
                }
                assignments.sort_by(|left, right| left.0.cmp(&right.0));
                Ok(assignments)
            }
        }
    }

    pub fn list_cross_community_edges(
        &self,
        community_by_symbol: &HashMap<String, i64>,
    ) -> Result<Vec<CrossCommunityEdge>, StoreError> {
        if community_by_symbol.is_empty() {
            return Ok(Vec::new());
        }

        let edges = self.list_dependency_edges_raw()?;
        let mut records = Vec::new();
        for (source_id, target_id, edge_kind) in edges {
            let Some(source_community) = community_by_symbol.get(source_id.as_str()) else {
                continue;
            };
            let Some(target_community) = community_by_symbol.get(target_id.as_str()) else {
                continue;
            };
            if source_community == target_community {
                continue;
            }
            records.push((
                source_id,
                target_id,
                edge_kind,
                *source_community,
                *target_community,
            ));
        }
        records.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        Ok(records)
    }

    pub fn list_pagerank(&self) -> Result<Vec<(String, f32)>, StoreError> {
        match self.try_pagerank_from_cozo() {
            Ok(mut records) => {
                records.sort_by(|left, right| {
                    right
                        .1
                        .partial_cmp(&left.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left.0.cmp(&right.0))
                });
                Ok(records)
            }
            Err(_) => self.list_pagerank_fallback(),
        }
    }

    pub fn list_betweenness_centrality(&self) -> Result<Vec<(String, f32)>, StoreError> {
        match self.try_betweenness_from_cozo() {
            Ok(mut records) => {
                records.sort_by(|left, right| {
                    right
                        .1
                        .partial_cmp(&left.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left.0.cmp(&right.0))
                });
                Ok(records)
            }
            Err(_) => self.list_betweenness_fallback(),
        }
    }

    pub fn list_strongly_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        match self.try_scc_from_cozo() {
            Ok(mut records) => {
                for component in &mut records {
                    component.sort();
                }
                records.sort_by(|left, right| {
                    right
                        .len()
                        .cmp(&left.len())
                        .then_with(|| left.join(",").cmp(&right.join(",")))
                });
                Ok(records)
            }
            Err(_) => self.list_scc_fallback(),
        }
    }

    pub fn list_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        match self.try_connected_components_from_cozo() {
            Ok(mut records) => {
                for component in &mut records {
                    component.sort();
                }
                records.sort_by(|left, right| {
                    right
                        .len()
                        .cmp(&left.len())
                        .then_with(|| left.join(",").cmp(&right.join(",")))
                });
                Ok(records)
            }
            Err(_) => self.list_connected_components_fallback(),
        }
    }

    fn list_connected_components_fallback(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let edges = self.list_dependency_edges_raw()?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        let mut adjacency = HashMap::<String, Vec<String>>::new();
        for (source, target, _) in &edges {
            adjacency
                .entry(source.clone())
                .or_default()
                .push(target.clone());
            adjacency
                .entry(target.clone())
                .or_default()
                .push(source.clone());
        }

        let mut visited = HashSet::new();
        let mut components = Vec::new();
        let mut nodes = adjacency.keys().cloned().collect::<Vec<_>>();
        nodes.sort();
        for node in nodes {
            if !visited.insert(node.clone()) {
                continue;
            }
            let mut queue = VecDeque::new();
            queue.push_back(node.clone());
            let mut component = vec![node];
            while let Some(current) = queue.pop_front() {
                if let Some(neighbors) = adjacency.get(current.as_str()) {
                    for neighbor in neighbors {
                        if visited.insert(neighbor.clone()) {
                            queue.push_back(neighbor.clone());
                            component.push(neighbor.clone());
                        }
                    }
                }
            }
            component.sort();
            component.dedup();
            components.push(component);
        }

        components.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.join(",").cmp(&right.join(",")))
        });
        Ok(components)
    }

    fn list_pagerank_fallback(&self) -> Result<Vec<(String, f32)>, StoreError> {
        let edges = self.list_dependency_edges_raw()?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        let mut nodes = HashSet::new();
        let mut incoming = HashMap::<String, Vec<String>>::new();
        let mut out_degree = HashMap::<String, usize>::new();
        for (source, target, _) in edges {
            nodes.insert(source.clone());
            nodes.insert(target.clone());
            incoming.entry(target).or_default().push(source.clone());
            *out_degree.entry(source).or_insert(0) += 1;
        }
        let mut nodes = nodes.into_iter().collect::<Vec<_>>();
        nodes.sort();
        let node_count = nodes.len();
        if node_count == 0 {
            return Ok(Vec::new());
        }

        let damping = 0.85f32;
        let node_count_f = node_count as f32;
        let mut rank = nodes
            .iter()
            .map(|node| (node.clone(), 1.0f32 / node_count_f))
            .collect::<HashMap<_, _>>();
        for _ in 0..25 {
            let dangling_sum = nodes
                .iter()
                .filter(|node| out_degree.get((*node).as_str()).copied().unwrap_or(0) == 0)
                .map(|node| rank.get(node.as_str()).copied().unwrap_or(0.0))
                .sum::<f32>();
            let mut next = HashMap::new();
            for node in &nodes {
                let incoming_sum = incoming
                    .get(node.as_str())
                    .into_iter()
                    .flatten()
                    .map(|source| {
                        let source_rank = rank.get(source.as_str()).copied().unwrap_or(0.0);
                        let degree = out_degree.get(source.as_str()).copied().unwrap_or(0).max(1);
                        source_rank / degree as f32
                    })
                    .sum::<f32>();
                let value = ((1.0 - damping) / node_count_f)
                    + damping * (incoming_sum + dangling_sum / node_count_f);
                next.insert(node.clone(), value);
            }
            rank = next;
        }

        let mut scored = rank.into_iter().collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        Ok(scored)
    }

    fn list_betweenness_fallback(&self) -> Result<Vec<(String, f32)>, StoreError> {
        let edges = self.list_dependency_edges_raw()?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut nodes = HashSet::new();
        for (source, target, _) in edges {
            nodes.insert(source.clone());
            nodes.insert(target.clone());
            adjacency.entry(source).or_default().push(target);
        }
        let mut node_ids = nodes.into_iter().collect::<Vec<_>>();
        node_ids.sort();
        for neighbors in adjacency.values_mut() {
            neighbors.sort();
            neighbors.dedup();
        }

        let mut centrality = node_ids
            .iter()
            .map(|node| (node.clone(), 0.0f32))
            .collect::<HashMap<_, _>>();

        for source in &node_ids {
            let mut stack = Vec::<String>::new();
            let mut predecessors = node_ids
                .iter()
                .map(|node| (node.clone(), Vec::<String>::new()))
                .collect::<HashMap<_, _>>();
            let mut sigma = node_ids
                .iter()
                .map(|node| (node.clone(), 0.0f32))
                .collect::<HashMap<_, _>>();
            let mut distance = node_ids
                .iter()
                .map(|node| (node.clone(), -1i32))
                .collect::<HashMap<_, _>>();

            sigma.insert(source.clone(), 1.0);
            distance.insert(source.clone(), 0);

            let mut queue = VecDeque::<String>::new();
            queue.push_back(source.clone());
            while let Some(vertex) = queue.pop_front() {
                stack.push(vertex.clone());
                let vertex_distance = distance.get(vertex.as_str()).copied().unwrap_or(-1);
                for neighbor in adjacency
                    .get(vertex.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    if distance.get(neighbor.as_str()).copied().unwrap_or(-1) < 0 {
                        distance.insert(neighbor.clone(), vertex_distance + 1);
                        queue.push_back(neighbor.clone());
                    }
                    if distance.get(neighbor.as_str()).copied().unwrap_or(-1) == vertex_distance + 1
                    {
                        let sigma_neighbor = sigma.get(neighbor.as_str()).copied().unwrap_or(0.0);
                        let sigma_vertex = sigma.get(vertex.as_str()).copied().unwrap_or(0.0);
                        sigma.insert(neighbor.clone(), sigma_neighbor + sigma_vertex);
                        predecessors
                            .entry(neighbor.clone())
                            .or_default()
                            .push(vertex.clone());
                    }
                }
            }

            let mut dependency = node_ids
                .iter()
                .map(|node| (node.clone(), 0.0f32))
                .collect::<HashMap<_, _>>();

            while let Some(vertex) = stack.pop() {
                let preds = predecessors
                    .get(vertex.as_str())
                    .cloned()
                    .unwrap_or_default();
                for predecessor in preds {
                    let sigma_vertex = sigma.get(vertex.as_str()).copied().unwrap_or(0.0);
                    if sigma_vertex <= f32::EPSILON {
                        continue;
                    }
                    let sigma_pred = sigma.get(predecessor.as_str()).copied().unwrap_or(0.0);
                    let dependency_vertex = dependency.get(vertex.as_str()).copied().unwrap_or(0.0);
                    let contribution = (sigma_pred / sigma_vertex) * (1.0 + dependency_vertex);
                    let previous = dependency.get(predecessor.as_str()).copied().unwrap_or(0.0);
                    dependency.insert(predecessor.clone(), previous + contribution);
                }
                if vertex != *source {
                    let previous = centrality.get(vertex.as_str()).copied().unwrap_or(0.0);
                    let contribution = dependency.get(vertex.as_str()).copied().unwrap_or(0.0);
                    centrality.insert(vertex, previous + contribution);
                }
            }
        }

        let node_count = node_ids.len() as f32;
        let normalizer = ((node_count - 1.0) * (node_count - 2.0)).max(1.0);
        let mut scored = centrality
            .into_iter()
            .map(|(node, value)| (node, (value / normalizer).clamp(0.0, 1.0)))
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        Ok(scored)
    }

    fn list_scc_fallback(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let edges = self.list_dependency_edges_raw()?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut reverse = HashMap::<String, Vec<String>>::new();
        let mut nodes = HashSet::new();
        for (source, target, _) in edges {
            nodes.insert(source.clone());
            nodes.insert(target.clone());
            adjacency
                .entry(source.clone())
                .or_default()
                .push(target.clone());
            reverse.entry(target).or_default().push(source);
        }
        let mut nodes = nodes.into_iter().collect::<Vec<_>>();
        nodes.sort();

        let mut visited = HashSet::<String>::new();
        let mut order = Vec::<String>::new();
        for node in &nodes {
            if visited.contains(node.as_str()) {
                continue;
            }
            dfs_postorder(node, &adjacency, &mut visited, &mut order);
        }

        let mut assigned = HashSet::<String>::new();
        let mut components = Vec::<Vec<String>>::new();
        while let Some(node) = order.pop() {
            if assigned.contains(node.as_str()) {
                continue;
            }
            let mut stack = vec![node.clone()];
            let mut component = Vec::new();
            assigned.insert(node);
            while let Some(current) = stack.pop() {
                component.push(current.clone());
                if let Some(neighbors) = reverse.get(current.as_str()) {
                    for neighbor in neighbors {
                        if assigned.insert(neighbor.clone()) {
                            stack.push(neighbor.clone());
                        }
                    }
                }
            }
            component.sort();
            components.push(component);
        }

        components.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.join(",").cmp(&right.join(",")))
        });
        Ok(components)
    }

    pub fn has_dependency_between_files(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(false);
        }

        let mut params = BTreeMap::new();
        params.insert("file_a".to_owned(), DataValue::from(file_a.to_owned()));
        params.insert("file_b".to_owned(), DataValue::from(file_b.to_owned()));

        let rows = self.run_script(
            r#"
            ?[source_id] :=
                *edges{source_id, target_id},
                *symbols{symbol_id: source_id, file_path: $file_a},
                *symbols{symbol_id: target_id, file_path: $file_b}
            ?[source_id] :=
                *edges{source_id, target_id},
                *symbols{symbol_id: source_id, file_path: $file_b},
                *symbols{symbol_id: target_id, file_path: $file_a}

            :limit 1
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        Ok(!rows.rows.is_empty())
    }

    pub fn list_upstream_dependency_traversal(
        &self,
        target_symbol_id: &str,
        max_depth: u32,
    ) -> Result<UpstreamDependencyTraversal, StoreError> {
        let target_symbol_id = target_symbol_id.trim();
        if target_symbol_id.is_empty() || max_depth == 0 {
            return Ok(UpstreamDependencyTraversal::default());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "start".to_owned(),
            DataValue::from(target_symbol_id.to_owned()),
        );
        params.insert("max_depth".to_owned(), DataValue::from(max_depth as i64));

        let rows = self.run_script(
            r#"
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind: "calls"}
            dep_edges[source, target] :=
                *edges{source_id: source, target_id: target, edge_kind: "depends_on"}

            reachable[source, target, depth] :=
                dep_edges[$start, target],
                source = $start,
                depth = 1
            reachable[source, target, depth] :=
                reachable[_, mid, prev_depth],
                prev_depth < $max_depth,
                dep_edges[mid, target],
                source = mid,
                depth = prev_depth + 1

            ?[source_id, target_id, depth] := reachable[source_id, target_id, depth]
            :order depth, source_id, target_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut seen_edges = BTreeSet::new();
        let mut edges = Vec::new();
        let mut min_depth_by_symbol = HashMap::<String, u32>::new();

        for row in rows.rows {
            if row.len() < 3 {
                return Err(StoreError::Cozo(
                    "invalid upstream traversal row shape".to_owned(),
                ));
            }
            let source_id = row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid upstream source_id value".to_owned()))?
                .to_owned();
            let target_id = row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid upstream target_id value".to_owned()))?
                .to_owned();
            let depth = row[2]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid upstream depth value".to_owned()))?;
            if depth <= 0 {
                continue;
            }
            let depth = depth as u32;
            if !seen_edges.insert((source_id.clone(), target_id.clone(), depth)) {
                continue;
            }

            edges.push(UpstreamDependencyEdgeRecord {
                source_id,
                target_id: target_id.clone(),
                depth,
            });
            min_depth_by_symbol
                .entry(target_id)
                .and_modify(|current| *current = (*current).min(depth))
                .or_insert(depth);
        }

        let mut nodes = min_depth_by_symbol
            .into_iter()
            .map(|(symbol_id, depth)| UpstreamDependencyNodeRecord { symbol_id, depth })
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });
        edges.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| left.target_id.cmp(&right.target_id))
        });

        Ok(UpstreamDependencyTraversal { nodes, edges })
    }

    pub fn upsert_co_change_edges(&self, records: &[CouplingEdgeRecord]) -> Result<(), StoreError> {
        for record in records {
            let mut params = BTreeMap::new();
            params.insert("file_a".to_owned(), DataValue::from(record.file_a.clone()));
            params.insert("file_b".to_owned(), DataValue::from(record.file_b.clone()));
            params.insert(
                "co_change_count".to_owned(),
                DataValue::from(record.co_change_count),
            );
            params.insert(
                "total_commits_a".to_owned(),
                DataValue::from(record.total_commits_a),
            );
            params.insert(
                "total_commits_b".to_owned(),
                DataValue::from(record.total_commits_b),
            );
            params.insert(
                "git_coupling".to_owned(),
                DataValue::from(record.git_coupling as f64),
            );
            params.insert(
                "static_signal".to_owned(),
                DataValue::from(record.static_signal as f64),
            );
            params.insert(
                "semantic_signal".to_owned(),
                DataValue::from(record.semantic_signal as f64),
            );
            params.insert(
                "fused_score".to_owned(),
                DataValue::from(record.fused_score as f64),
            );
            params.insert(
                "coupling_type".to_owned(),
                DataValue::from(record.coupling_type.clone()),
            );
            params.insert(
                "last_co_change_commit".to_owned(),
                DataValue::from(record.last_co_change_commit.clone()),
            );
            params.insert(
                "last_co_change_at".to_owned(),
                DataValue::from(record.last_co_change_at),
            );
            params.insert("mined_at".to_owned(), DataValue::from(record.mined_at));

            self.run_script(
                r#"
                ?[
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                ] <- [[
                    $file_a,
                    $file_b,
                    $co_change_count,
                    $total_commits_a,
                    $total_commits_b,
                    $git_coupling,
                    $static_signal,
                    $semantic_signal,
                    $fused_score,
                    $coupling_type,
                    $last_co_change_commit,
                    $last_co_change_at,
                    $mined_at
                ]]
                :put co_change_edges {
                    file_a,
                    file_b =>
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                }
                "#,
                params,
                ScriptMutability::Mutable,
            )?;
        }

        Ok(())
    }

    pub fn get_co_change_edge(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<Option<CouplingEdgeRecord>, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(None);
        }

        let mut params = BTreeMap::new();
        params.insert("file_a".to_owned(), DataValue::from(file_a.to_owned()));
        params.insert("file_b".to_owned(), DataValue::from(file_b.to_owned()));
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_a = $file_a,
                file_b = $file_b
            :limit 1
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .first()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .transpose()
    }

    pub fn list_co_change_edges_for_file(
        &self,
        file_path: &str,
        min_fused_score: f32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "file_path".to_owned(),
            DataValue::from(file_path.to_owned()),
        );
        params.insert(
            "min_fused_score".to_owned(),
            DataValue::from(min_fused_score as f64),
        );
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_a = $file_path,
                fused_score >= $min_fused_score
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                },
                file_b = $file_path,
                fused_score >= $min_fused_score
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut edges = rows
            .rows
            .iter()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        edges.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        Ok(edges)
    }

    pub fn list_top_co_change_edges(
        &self,
        limit: u32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let rows = self.run_script(
            r#"
            ?[
                file_a,
                file_b,
                co_change_count,
                total_commits_a,
                total_commits_b,
                git_coupling,
                static_signal,
                semantic_signal,
                fused_score,
                coupling_type,
                last_co_change_commit,
                last_co_change_at,
                mined_at
            ] :=
                *co_change_edges{
                    file_a,
                    file_b,
                    co_change_count,
                    total_commits_a,
                    total_commits_b,
                    git_coupling,
                    static_signal,
                    semantic_signal,
                    fused_score,
                    coupling_type,
                    last_co_change_commit,
                    last_co_change_at,
                    mined_at
                }
            "#,
            BTreeMap::new(),
            ScriptMutability::Immutable,
        )?;

        let mut edges = rows
            .rows
            .iter()
            .map(|row| Self::row_to_coupling_edge(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        edges.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        edges.truncate(limit.clamp(1, 200) as usize);
        Ok(edges)
    }

    pub fn replace_tested_by_for_test_file(
        &self,
        test_file: &str,
        records: &[TestedByRecord],
    ) -> Result<(), StoreError> {
        let test_file = test_file.trim();
        if test_file.is_empty() {
            return Ok(());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "test_file".to_owned(),
            DataValue::from(test_file.to_owned()),
        );
        self.run_script(
            r#"
            ?[target_file, test_file] :=
                *tested_by{
                    target_file,
                    test_file,
                    intent_count,
                    confidence,
                    inference_method
                },
                test_file = $test_file
            :rm tested_by { target_file, test_file }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        for record in records {
            let mut params = BTreeMap::new();
            params.insert(
                "target_file".to_owned(),
                DataValue::from(record.target_file.clone()),
            );
            params.insert(
                "test_file".to_owned(),
                DataValue::from(record.test_file.clone()),
            );
            params.insert(
                "intent_count".to_owned(),
                DataValue::from(record.intent_count.max(0)),
            );
            params.insert(
                "confidence".to_owned(),
                DataValue::from(record.confidence.clamp(0.0, 1.0) as f64),
            );
            params.insert(
                "inference_method".to_owned(),
                DataValue::from(record.inference_method.clone()),
            );

            self.run_script(
                r#"
                ?[target_file, test_file, intent_count, confidence, inference_method] <- [[
                    $target_file,
                    $test_file,
                    $intent_count,
                    $confidence,
                    $inference_method
                ]]
                :put tested_by {
                    target_file,
                    test_file =>
                    intent_count,
                    confidence,
                    inference_method
                }
                "#,
                params,
                ScriptMutability::Mutable,
            )?;
        }

        Ok(())
    }

    pub fn list_tested_by_for_target_file(
        &self,
        target_file: &str,
    ) -> Result<Vec<TestedByRecord>, StoreError> {
        let target_file = target_file.trim();
        if target_file.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "target_file".to_owned(),
            DataValue::from(target_file.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                target_file,
                test_file,
                intent_count,
                confidence,
                inference_method
            ] :=
                *tested_by{
                    target_file,
                    test_file,
                    intent_count,
                    confidence,
                    inference_method
                },
                target_file = $target_file
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut records = rows
            .rows
            .iter()
            .map(|row| Self::row_to_tested_by(row.as_slice()))
            .collect::<Result<Vec<_>, _>>()?;
        records.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.test_file.cmp(&right.test_file))
        });
        Ok(records)
    }
}

impl GraphStore for CozoGraphStore {
    fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert("symbol_id".to_owned(), DataValue::from(symbol.id.clone()));
        params.insert(
            "qualified_name".to_owned(),
            DataValue::from(symbol.qualified_name.clone()),
        );
        params.insert(
            "name".to_owned(),
            DataValue::from(Self::symbol_name(&symbol.qualified_name).to_owned()),
        );
        params.insert("kind".to_owned(), DataValue::from(symbol.kind.clone()));
        params.insert(
            "file_path".to_owned(),
            DataValue::from(symbol.file_path.clone()),
        );
        params.insert(
            "language".to_owned(),
            DataValue::from(symbol.language.clone()),
        );
        params.insert(
            "signature_fingerprint".to_owned(),
            DataValue::from(symbol.signature_fingerprint.clone()),
        );
        params.insert(
            "last_seen_at".to_owned(),
            DataValue::from(symbol.last_seen_at),
        );

        self.run_script(
            r#"
            ?[
                symbol_id,
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            ] <- [[
                $symbol_id,
                $qualified_name,
                $name,
                $kind,
                $file_path,
                $language,
                $signature_fingerprint,
                $last_seen_at
            ]]
            :put symbols {
                symbol_id =>
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(edge.source_id.clone()),
        );
        params.insert(
            "target_id".to_owned(),
            DataValue::from(edge.target_id.clone()),
        );
        params.insert(
            "edge_kind".to_owned(),
            DataValue::from(edge.edge_kind.as_str().to_owned()),
        );
        params.insert(
            "file_path".to_owned(),
            DataValue::from(edge.file_path.clone()),
        );

        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind, file_path] <- [[
                $source_id,
                $target_id,
                $edge_kind,
                $file_path
            ]]
            :put edges { source_id, target_id, edge_kind => file_path }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "qname".to_owned(),
            DataValue::from(qualified_name.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: symbol_id, target_id, edge_kind: "calls"},
                *symbols{symbol_id: target_id, qualified_name: $qname},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(symbol_id.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: $source_id, target_id: symbol_id, edge_kind: "calls"},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert("start".to_owned(), DataValue::from(symbol_id.to_owned()));
        params.insert("max_depth".to_owned(), DataValue::from(depth as i64));
        let rows = self.run_script(
            r#"
            reachable[node, depth] :=
                *edges{source_id: $start, target_id: node, edge_kind: "calls"},
                depth = 1
            reachable[node, depth] :=
                reachable[prev, prev_depth],
                prev_depth < $max_depth,
                *edges{source_id: prev, target_id: node, edge_kind: "calls"},
                depth = prev_depth + 1

            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at,
                depth
            ] :=
                reachable[symbol_id, depth],
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order depth, qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut levels = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows.rows {
            if row.len() < 8 {
                return Err(StoreError::Cozo("invalid call chain row shape".to_owned()));
            }
            let record = Self::row_to_symbol(&row[..7])?;
            if !seen.insert(record.id.clone()) {
                continue;
            }
            let depth = row[7]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid depth value".to_owned()))?;
            if depth <= 0 {
                continue;
            }
            let depth_idx = depth as usize - 1;
            while levels.len() <= depth_idx {
                levels.push(Vec::new());
            }
            if let Some(level) = levels.get_mut(depth_idx) {
                level.push(record);
            }
        }

        Ok(levels)
    }

    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "file_path".to_owned(),
            DataValue::from(file_path.to_owned()),
        );
        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind] :=
                *edges{source_id, target_id, edge_kind, file_path: $file_path}

            :rm edges { source_id, target_id, edge_kind }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }
}

fn data_value_to_i64(value: &DataValue) -> Option<i64> {
    if let Some(raw) = value.get_int() {
        return Some(raw);
    }
    value.get_float().map(|raw| raw as i64)
}

fn dfs_postorder(
    start: &str,
    adjacency: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    order: &mut Vec<String>,
) {
    let mut stack = vec![(start.to_owned(), false)];
    while let Some((node, expanded)) = stack.pop() {
        if expanded {
            order.push(node);
            continue;
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        stack.push((node.clone(), true));
        if let Some(neighbors) = adjacency.get(node.as_str()) {
            for neighbor in neighbors {
                if !visited.contains(neighbor.as_str()) {
                    stack.push((neighbor.clone(), false));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{Store, SymbolRecord};
    use aether_core::{EdgeKind, SymbolEdge};

    fn symbol(id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[test]
    fn cozo_graph_returns_multi_hop_call_chain() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        let delta = symbol("sym-delta", "delta");
        for row in [&alpha, &beta, &gamma, &delta] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }

        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: delta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let chain = graph
            .get_call_chain(&alpha.id, 3)
            .expect("get call chain at depth 3");
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0][0].id, beta.id);
        assert_eq!(chain[1][0].id, gamma.id);
        assert_eq!(chain[2][0].id, delta.id);
    }

    #[test]
    fn unresolved_edges_are_skipped_during_sync() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: alpha.id.clone(),
                target_qualified_name: "missing::target".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            }])
            .expect("upsert unresolved edge");

        let stats = store
            .sync_graph_for_file(&graph, "src/lib.rs")
            .expect("sync graph for file");
        assert_eq!(stats.resolved_edges, 0);
        assert_eq!(stats.unresolved_edges, 1);

        let deps = graph
            .get_dependencies(&alpha.id)
            .expect("query dependencies after unresolved sync");
        assert!(deps.is_empty());
    }

    #[test]
    fn cozo_graph_stores_and_queries_co_change_edges() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        graph
            .upsert_co_change_edges(&[CouplingEdgeRecord {
                file_a: "src/a.rs".to_owned(),
                file_b: "src/b.rs".to_owned(),
                co_change_count: 4,
                total_commits_a: 6,
                total_commits_b: 7,
                git_coupling: 4.0 / 7.0,
                static_signal: 1.0,
                semantic_signal: 0.7,
                fused_score: 0.5,
                coupling_type: "multi".to_owned(),
                last_co_change_commit: "abc123".to_owned(),
                last_co_change_at: 1_700_000_000,
                mined_at: 1_700_000_100,
            }])
            .expect("upsert co change edge");

        let top = graph
            .list_top_co_change_edges(10)
            .expect("list top co change edges");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].file_a, "src/a.rs");
        assert_eq!(top[0].file_b, "src/b.rs");

        let direct = graph
            .get_co_change_edge("src/a.rs", "src/b.rs")
            .expect("get direct co change edge");
        assert!(direct.is_some());

        let neighbors = graph
            .list_co_change_edges_for_file("src/a.rs", 0.2)
            .expect("list neighbors");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].coupling_type, "multi");
    }

    #[test]
    fn cozo_graph_stores_and_queries_tested_by_edges() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        graph
            .replace_tested_by_for_test_file(
                "tests/payment_test.rs",
                &[
                    TestedByRecord {
                        target_file: "src/payment.rs".to_owned(),
                        test_file: "tests/payment_test.rs".to_owned(),
                        intent_count: 3,
                        confidence: 0.9,
                        inference_method: "naming_convention".to_owned(),
                    },
                    TestedByRecord {
                        target_file: "src/ledger.rs".to_owned(),
                        test_file: "tests/payment_test.rs".to_owned(),
                        intent_count: 1,
                        confidence: 0.4,
                        inference_method: "coupling_cross_reference".to_owned(),
                    },
                ],
            )
            .expect("replace tested_by edges");

        let guards = graph
            .list_tested_by_for_target_file("src/payment.rs")
            .expect("list tested_by for target");
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].test_file, "tests/payment_test.rs");
        assert_eq!(guards[0].intent_count, 3);
        assert_eq!(guards[0].inference_method, "naming_convention");
    }

    #[test]
    fn cozo_graph_graph_algorithms_return_expected_shapes() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        for row in [&alpha, &beta, &gamma] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }
        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: alpha.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let communities = graph
            .list_louvain_communities()
            .expect("louvain communities");
        assert!(!communities.is_empty());

        let pagerank = graph.list_pagerank().expect("pagerank");
        assert!(!pagerank.is_empty());

        let betweenness = graph
            .list_betweenness_centrality()
            .expect("betweenness centrality");
        assert!(!betweenness.is_empty());

        let scc = graph
            .list_strongly_connected_components()
            .expect("scc components");
        assert!(!scc.is_empty());
        assert!(
            scc.iter()
                .any(|component| { component.contains(&alpha.id) && component.contains(&beta.id) })
        );

        let connected = graph
            .list_connected_components()
            .expect("connected components");
        assert!(!connected.is_empty());
    }

    #[test]
    fn cozo_graph_betweenness_identifies_bottleneck_node() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let a = symbol("sym-a", "a");
        let b = symbol("sym-b", "b");
        let c = symbol("sym-c", "c");
        let d = symbol("sym-d", "d");
        for node in [&a, &b, &c, &d] {
            graph.upsert_symbol_node(node).expect("upsert symbol");
        }

        for edge in [
            ResolvedEdge {
                source_id: a.id.clone(),
                target_id: b.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: b.id.clone(),
                target_id: c.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: b.id.clone(),
                target_id: d.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let scores = graph
            .list_betweenness_centrality()
            .expect("betweenness scores");
        assert!(!scores.is_empty());
        assert_eq!(scores[0].0, b.id);
        assert!(scores[0].1 > 0.0);
    }

    #[test]
    fn cozo_graph_upstream_traversal_returns_nodes_edges_and_depths() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        let delta = symbol("sym-delta", "delta");
        for row in [&alpha, &beta, &gamma, &delta] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }
        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: delta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let traversal = graph
            .list_upstream_dependency_traversal(&alpha.id, 3)
            .expect("upstream traversal");
        let by_id = traversal
            .nodes
            .iter()
            .map(|node| (node.symbol_id.clone(), node.depth))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(by_id.get(beta.id.as_str()).copied(), Some(1));
        assert_eq!(by_id.get(gamma.id.as_str()).copied(), Some(2));
        assert_eq!(by_id.get(delta.id.as_str()).copied(), Some(2));

        assert!(traversal.edges.iter().any(|edge| {
            edge.source_id == alpha.id && edge.target_id == beta.id && edge.depth == 1
        }));
        assert!(traversal.edges.iter().any(|edge| {
            edge.source_id == beta.id && edge.target_id == gamma.id && edge.depth == 2
        }));
    }

    #[test]
    fn cozo_graph_upstream_traversal_respects_depth_limit_with_cycles() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        for row in [&alpha, &beta, &gamma] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }
        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let traversal = graph
            .list_upstream_dependency_traversal(&alpha.id, 3)
            .expect("upstream traversal");
        assert!(traversal.nodes.iter().all(|node| node.depth <= 3));
        assert!(traversal.edges.iter().all(|edge| edge.depth <= 3));
    }
}
