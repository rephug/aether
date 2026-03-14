use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use aether_core::EdgeKind;
use aether_graph_algo::{
    GraphAlgorithmEdge, connected_components_sync, louvain_sync, page_rank_sync,
    strongly_connected_components_sync,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use surrealdb::Surreal;
use surrealdb::engine::local::{Db, SurrealKv};

use super::{
    CouplingEdgeRecord, GraphDependencyEdgeRecord, GraphStore, ResolvedEdge, STRUCTURAL_EDGE_KINDS,
    StoreError, SymbolRecord, TestedByRecord, UpstreamDependencyEdgeRecord,
    UpstreamDependencyNodeRecord, UpstreamDependencyTraversal,
};

pub type CrossCommunityEdge = (String, String, String, i64, i64);

#[derive(Clone)]
pub struct SurrealGraphStore {
    db: Surreal<Db>,
}

impl SurrealGraphStore {
    pub async fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let graph_dir = workspace_root.join(".aether").join("graph");
        fs::create_dir_all(&graph_dir)?;

        let db = if let Some(existing) = cached_surreal_handle(&graph_dir)? {
            existing
        } else {
            let mut open_error = None;
            let mut db_opt = None;
            for attempt in 0..10 {
                match tokio::time::timeout(
                    Duration::from_secs(5),
                    Surreal::new::<SurrealKv>(graph_dir.clone()),
                )
                .await
                {
                    Ok(Ok(db)) => {
                        db_opt = Some(db);
                        break;
                    }
                    Ok(Err(err)) => {
                        let message = err.to_string();
                        if message.contains("LOCK is already locked") && attempt < 9 {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            continue;
                        }
                        open_error =
                            Some(StoreError::Graph(format!("SurrealDB open failed: {err}")));
                        break;
                    }
                    Err(_) => {
                        open_error = Some(StoreError::Graph(
                            "SurrealDB open timed out (is another aetherd process holding the lock?)"
                                .to_owned(),
                        ));
                        break;
                    }
                }
            }
            let db = if let Some(db) = db_opt {
                db
            } else {
                return Err(open_error.unwrap_or_else(|| {
                    StoreError::Graph("SurrealDB open failed: unknown error".to_owned())
                }));
            };
            cache_surreal_handle(graph_dir.clone(), db.clone())?;
            db
        };
        db.use_ns("aether")
            .use_db("graph")
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB namespace setup failed: {err}")))?;

        let store = Self { db };
        if !is_schema_initialized(&graph_dir)? {
            store.ensure_schema().await?;
            mark_schema_initialized(graph_dir.clone())?;
        }
        Ok(store)
    }

    pub async fn open_readonly(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open(workspace_root).await
    }

    pub fn db(&self) -> &Surreal<Db> {
        &self.db
    }

    async fn ensure_schema(&self) -> Result<(), StoreError> {
        let schema = r#"
            DEFINE TABLE IF NOT EXISTS symbol SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS symbol_id ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS qualified_name ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS name ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS kind ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS file_path ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS language ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS signature_fingerprint ON symbol TYPE string;
            DEFINE FIELD IF NOT EXISTS last_seen_at ON symbol TYPE int;
            DEFINE FIELD IF NOT EXISTS updated_at ON symbol TYPE datetime DEFAULT time::now();
            DEFINE INDEX IF NOT EXISTS idx_symbol_symbol_id ON symbol FIELDS symbol_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_symbol_qualified_name ON symbol FIELDS qualified_name;
            DEFINE INDEX IF NOT EXISTS idx_symbol_file_path ON symbol FIELDS file_path;

            DEFINE TABLE IF NOT EXISTS depends_on SCHEMAFULL TYPE RELATION FROM symbol TO symbol;
            DEFINE FIELD IF NOT EXISTS edge_kind ON depends_on TYPE string;
            DEFINE FIELD IF NOT EXISTS file_path ON depends_on TYPE string;
            DEFINE FIELD IF NOT EXISTS source_symbol_id ON depends_on TYPE string;
            DEFINE FIELD IF NOT EXISTS target_symbol_id ON depends_on TYPE string;
            DEFINE FIELD IF NOT EXISTS weight ON depends_on TYPE float DEFAULT 1.0;
            DEFINE FIELD IF NOT EXISTS in ON depends_on TYPE record<symbol> REFERENCE;
            DEFINE FIELD IF NOT EXISTS out ON depends_on TYPE record<symbol> REFERENCE;
            DEFINE INDEX IF NOT EXISTS idx_depends_on_file_path ON depends_on FIELDS file_path;

            DEFINE TABLE IF NOT EXISTS co_change SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS file_a ON co_change TYPE string;
            DEFINE FIELD IF NOT EXISTS file_b ON co_change TYPE string;
            DEFINE FIELD IF NOT EXISTS co_change_count ON co_change TYPE int;
            DEFINE FIELD IF NOT EXISTS total_commits_a ON co_change TYPE int;
            DEFINE FIELD IF NOT EXISTS total_commits_b ON co_change TYPE int;
            DEFINE FIELD IF NOT EXISTS git_coupling ON co_change TYPE float;
            DEFINE FIELD IF NOT EXISTS static_signal ON co_change TYPE float;
            DEFINE FIELD IF NOT EXISTS semantic_signal ON co_change TYPE float;
            DEFINE FIELD IF NOT EXISTS fused_score ON co_change TYPE float;
            DEFINE FIELD IF NOT EXISTS coupling_type ON co_change TYPE string;
            DEFINE FIELD IF NOT EXISTS last_co_change_commit ON co_change TYPE string;
            DEFINE FIELD IF NOT EXISTS last_co_change_at ON co_change TYPE int;
            DEFINE FIELD IF NOT EXISTS mined_at ON co_change TYPE int;
            DEFINE INDEX IF NOT EXISTS idx_co_change_pair ON co_change FIELDS file_a, file_b UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_co_change_score ON co_change FIELDS fused_score;

            DEFINE TABLE IF NOT EXISTS tested_by SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS target_file ON tested_by TYPE string;
            DEFINE FIELD IF NOT EXISTS test_file ON tested_by TYPE string;
            DEFINE FIELD IF NOT EXISTS intent_count ON tested_by TYPE int;
            DEFINE FIELD IF NOT EXISTS confidence ON tested_by TYPE float;
            DEFINE FIELD IF NOT EXISTS inference_method ON tested_by TYPE string;
            DEFINE INDEX IF NOT EXISTS idx_tested_by_pair ON tested_by FIELDS target_file, test_file UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_tested_by_target ON tested_by FIELDS target_file;
            DEFINE INDEX IF NOT EXISTS idx_tested_by_test ON tested_by FIELDS test_file;

            DEFINE TABLE IF NOT EXISTS community_snapshot SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS snapshot_id ON community_snapshot TYPE string;
            DEFINE FIELD IF NOT EXISTS community_id ON community_snapshot TYPE int;
            DEFINE FIELD IF NOT EXISTS members ON community_snapshot TYPE array;
            DEFINE FIELD IF NOT EXISTS created_at ON community_snapshot TYPE datetime DEFAULT time::now();

            -- Document unit nodes (domain-scoped, parallel to code symbol table)
            DEFINE TABLE IF NOT EXISTS document_node SCHEMAFULL;
            DEFINE FIELD IF NOT EXISTS unit_id ON document_node TYPE string;
            DEFINE FIELD IF NOT EXISTS domain ON document_node TYPE string;
            DEFINE FIELD IF NOT EXISTS unit_kind ON document_node TYPE string;
            DEFINE FIELD IF NOT EXISTS display_name ON document_node TYPE string;
            DEFINE FIELD IF NOT EXISTS source_path ON document_node TYPE string;
            DEFINE INDEX IF NOT EXISTS idx_doc_node_id ON document_node FIELDS unit_id UNIQUE;
            DEFINE INDEX IF NOT EXISTS idx_doc_node_domain ON document_node FIELDS domain;

            -- Document edges (domain-scoped, typed relationships)
            DEFINE TABLE IF NOT EXISTS document_edge SCHEMAFULL TYPE RELATION
                FROM document_node TO document_node;
            DEFINE FIELD IF NOT EXISTS edge_type ON document_edge TYPE string;
            DEFINE FIELD IF NOT EXISTS domain ON document_edge TYPE string;
            DEFINE FIELD IF NOT EXISTS weight ON document_edge TYPE float DEFAULT 1.0;
            DEFINE FIELD IF NOT EXISTS metadata_json ON document_edge TYPE string DEFAULT '{}';
            DEFINE FIELD IF NOT EXISTS in ON document_edge TYPE record<document_node> REFERENCE;
            DEFINE FIELD IF NOT EXISTS out ON document_edge TYPE record<document_node> REFERENCE;

            DEFINE FIELD IF NOT EXISTS callers ON symbol COMPUTED <~depends_on;
            DEFINE FIELD IF NOT EXISTS dependees ON symbol COMPUTED ->depends_on->symbol;
        "#;

        self.db
            .query(schema)
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB schema setup failed: {err}")))?;
        Ok(())
    }

    pub async fn list_louvain_communities(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let edges = self.list_dependency_edges_raw().await?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        let algo_edges = to_algo_edges(edges);
        let assignments = tokio::task::spawn_blocking(move || louvain_sync(&algo_edges))
            .await
            .map_err(|err| StoreError::Graph(format!("spawn_blocking louvain failed: {err}")))?;
        let mut records = assignments
            .into_iter()
            .map(|(node, community)| (node, community as i64))
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(records)
    }

    pub async fn list_cross_community_edges(
        &self,
        community_by_symbol: &HashMap<String, i64>,
    ) -> Result<Vec<CrossCommunityEdge>, StoreError> {
        if community_by_symbol.is_empty() {
            return Ok(Vec::new());
        }
        let edges = self.list_dependency_edges_raw().await?;
        let mut records = Vec::new();
        for edge in edges {
            let Some(source_community) = community_by_symbol.get(edge.source_id.as_str()) else {
                continue;
            };
            let Some(target_community) = community_by_symbol.get(edge.target_id.as_str()) else {
                continue;
            };
            if source_community == target_community {
                continue;
            }
            records.push((
                edge.source_id,
                edge.target_id,
                edge.edge_kind,
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

    pub async fn list_pagerank(&self) -> Result<Vec<(String, f32)>, StoreError> {
        let edges = self.list_dependency_edges_raw().await?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        let algo_edges = to_algo_edges(edges);
        let scores = tokio::task::spawn_blocking(move || page_rank_sync(&algo_edges, 0.85, 25))
            .await
            .map_err(|err| StoreError::Graph(format!("spawn_blocking pagerank failed: {err}")))?;
        Ok(scores
            .into_iter()
            .map(|(node, score)| (node, score as f32))
            .collect())
    }

    pub async fn list_strongly_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let edges = self.list_dependency_edges_raw().await?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        let algo_edges = to_algo_edges(edges);
        tokio::task::spawn_blocking(move || strongly_connected_components_sync(&algo_edges))
            .await
            .map_err(|err| StoreError::Graph(format!("spawn_blocking scc failed: {err}")))
    }

    pub async fn list_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        let edges = self.list_dependency_edges_raw().await?;
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        let algo_edges = to_algo_edges(edges);
        tokio::task::spawn_blocking(move || connected_components_sync(&algo_edges))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("spawn_blocking connected_components failed: {err}"))
            })
    }

    pub async fn list_all_symbol_ids(&self) -> Result<Vec<String>, StoreError> {
        let mut symbol_ids = self
            .query_rows::<String>("SELECT VALUE symbol_id FROM symbol;")
            .await?;
        symbol_ids.retain(|value| !value.trim().is_empty());
        symbol_ids.sort();
        symbol_ids.dedup();
        Ok(symbol_ids)
    }

    pub async fn list_existing_symbol_ids(
        &self,
        symbol_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        if symbol_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut ids = symbol_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut response = self
            .db
            .query("SELECT VALUE symbol_id FROM symbol WHERE symbol_id INSIDE $ids;")
            .bind(("ids", ids))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB list existing symbol IDs failed: {err}"))
            })?;
        let mut rows: Vec<String> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB list existing symbol IDs decode failed: {err}"
            ))
        })?;
        rows.retain(|value| !value.trim().is_empty());
        rows.sort();
        rows.dedup();
        Ok(rows)
    }

    pub async fn list_dependency_edges(
        &self,
    ) -> Result<Vec<GraphDependencyEdgeRecord>, StoreError> {
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    source_symbol_id: source_symbol_id,
                    target_symbol_id: target_symbol_id,
                    edge_kind: edge_kind
                }
                FROM depends_on;
                "#,
            )
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB list dependency edges failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB list dependency edges decode failed: {err}"
            ))
        })?;
        let mut edges = decode_rows::<GraphDependencyEdgeRecord>(rows)?;
        edges.retain(|edge| !edge.source_symbol_id.is_empty() && !edge.target_symbol_id.is_empty());
        edges.sort_by(|left, right| {
            left.source_symbol_id
                .cmp(&right.source_symbol_id)
                .then_with(|| left.target_symbol_id.cmp(&right.target_symbol_id))
                .then_with(|| left.edge_kind.cmp(&right.edge_kind))
        });
        Ok(edges)
    }

    pub async fn delete_symbol_by_symbol_id(&self, symbol_id: &str) -> Result<(), StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(());
        }
        self.db
            .query("DELETE symbol WHERE symbol_id = $symbol_id;")
            .bind(("symbol_id", symbol_id.to_owned()))
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB delete symbol failed: {err}")))?;
        Ok(())
    }

    pub async fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        let mut ids = symbol_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            return Ok(());
        }

        for chunk in ids.chunks(500) {
            let symbol_ids = chunk.to_vec();
            self.db
                .query(
                    r#"
                    DELETE depends_on
                    WHERE source_symbol_id INSIDE $symbol_ids
                       OR target_symbol_id INSIDE $symbol_ids;
                    DELETE symbol WHERE symbol_id INSIDE $symbol_ids;
                    "#,
                )
                .bind(("symbol_ids", symbol_ids))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!("SurrealDB delete symbols batch failed: {err}"))
                })?;
        }

        Ok(())
    }

    pub async fn delete_dependency_edges_by_pair(
        &self,
        source_symbol_id: &str,
        target_symbol_id: &str,
    ) -> Result<(), StoreError> {
        let source_symbol_id = source_symbol_id.trim();
        let target_symbol_id = target_symbol_id.trim();
        if source_symbol_id.is_empty() || target_symbol_id.is_empty() {
            return Ok(());
        }

        self.db
            .query(
                r#"
                DELETE depends_on
                WHERE source_symbol_id = $source_symbol_id
                  AND target_symbol_id = $target_symbol_id;
                "#,
            )
            .bind(("source_symbol_id", source_symbol_id.to_owned()))
            .bind(("target_symbol_id", target_symbol_id.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB delete dependency edges failed: {err}"))
            })?;
        Ok(())
    }

    pub async fn has_dependency_between_files(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(false);
        }
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE 1
                FROM depends_on
                WHERE (in.file_path = $file_a AND out.file_path = $file_b)
                   OR (in.file_path = $file_b AND out.file_path = $file_a)
                LIMIT 1;
                "#,
            )
            .bind(("file_a", file_a.to_owned()))
            .bind(("file_b", file_b.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB has_dependency query failed: {err}"))
            })?;

        let rows: Vec<i64> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB has_dependency decode failed: {err}"))
        })?;

        Ok(!rows.is_empty())
    }

    pub async fn list_upstream_dependency_traversal(
        &self,
        target_symbol_id: &str,
        max_depth: u32,
    ) -> Result<UpstreamDependencyTraversal, StoreError> {
        let target_symbol_id = target_symbol_id.trim();
        if target_symbol_id.is_empty() || max_depth == 0 {
            return Ok(UpstreamDependencyTraversal::default());
        }

        let mut seen_edges = BTreeSet::<(String, String, u32)>::new();
        let mut min_depth_by_symbol = HashMap::<String, u32>::new();
        let mut frontier = BTreeSet::<String>::new();
        frontier.insert(target_symbol_id.to_owned());

        for depth in 1..=max_depth {
            if frontier.is_empty() {
                break;
            }

            let sources = frontier.iter().cloned().collect::<Vec<_>>();
            let edges = self
                .list_dependency_edges_for_sources_by_kind(
                    sources.as_slice(),
                    STRUCTURAL_EDGE_KINDS,
                )
                .await?;
            if edges.is_empty() {
                break;
            }

            let mut next = BTreeSet::<String>::new();
            for edge in edges {
                let target = edge.target_id;
                seen_edges.insert((edge.source_id, target.clone(), depth));
                min_depth_by_symbol
                    .entry(target.clone())
                    .and_modify(|current| *current = (*current).min(depth))
                    .or_insert(depth);
                next.insert(target);
            }
            frontier = next;
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

        let mut edges = seen_edges
            .into_iter()
            .map(
                |(source_id, target_id, depth)| UpstreamDependencyEdgeRecord {
                    source_id,
                    target_id,
                    depth,
                },
            )
            .collect::<Vec<_>>();
        edges.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| left.target_id.cmp(&right.target_id))
        });

        Ok(UpstreamDependencyTraversal { nodes, edges })
    }

    pub async fn upsert_co_change_edges(
        &self,
        records: &[CouplingEdgeRecord],
    ) -> Result<(), StoreError> {
        for record in records {
            let record = CouplingEdgeRecord {
                file_a: record.file_a.clone(),
                file_b: record.file_b.clone(),
                co_change_count: record.co_change_count,
                total_commits_a: record.total_commits_a,
                total_commits_b: record.total_commits_b,
                git_coupling: record.git_coupling,
                static_signal: record.static_signal,
                semantic_signal: record.semantic_signal,
                fused_score: record.fused_score,
                coupling_type: record.coupling_type.clone(),
                last_co_change_commit: record.last_co_change_commit.clone(),
                last_co_change_at: record.last_co_change_at,
                mined_at: record.mined_at,
            };

            self.db
                .query(
                    r#"
                    DELETE co_change WHERE file_a = $file_a AND file_b = $file_b;
                    CREATE co_change SET
                        file_a = $file_a,
                        file_b = $file_b,
                        co_change_count = $co_change_count,
                        total_commits_a = $total_commits_a,
                        total_commits_b = $total_commits_b,
                        git_coupling = $git_coupling,
                        static_signal = $static_signal,
                        semantic_signal = $semantic_signal,
                        fused_score = $fused_score,
                        coupling_type = $coupling_type,
                        last_co_change_commit = $last_co_change_commit,
                        last_co_change_at = $last_co_change_at,
                        mined_at = $mined_at;
                    "#,
                )
                .bind(("file_a", record.file_a))
                .bind(("file_b", record.file_b))
                .bind(("co_change_count", record.co_change_count))
                .bind(("total_commits_a", record.total_commits_a))
                .bind(("total_commits_b", record.total_commits_b))
                .bind(("git_coupling", record.git_coupling))
                .bind(("static_signal", record.static_signal))
                .bind(("semantic_signal", record.semantic_signal))
                .bind(("fused_score", record.fused_score))
                .bind(("coupling_type", record.coupling_type))
                .bind(("last_co_change_commit", record.last_co_change_commit))
                .bind(("last_co_change_at", record.last_co_change_at))
                .bind(("mined_at", record.mined_at))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!("SurrealDB upsert co_change failed: {err}"))
                })?;
        }
        Ok(())
    }

    pub async fn get_co_change_edge(
        &self,
        source_file: &str,
        target_file: &str,
    ) -> Result<Option<CouplingEdgeRecord>, StoreError> {
        let source_file = source_file.trim();
        let target_file = target_file.trim();
        if source_file.is_empty() || target_file.is_empty() {
            return Ok(None);
        }
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    file_a: file_a,
                    file_b: file_b,
                    co_change_count: co_change_count,
                    total_commits_a: total_commits_a,
                    total_commits_b: total_commits_b,
                    git_coupling: git_coupling,
                    static_signal: static_signal,
                    semantic_signal: semantic_signal,
                    fused_score: fused_score,
                    coupling_type: coupling_type,
                    last_co_change_commit: last_co_change_commit,
                    last_co_change_at: last_co_change_at,
                    mined_at: mined_at
                }
                FROM co_change
                WHERE file_a = $file_a AND file_b = $file_b
                LIMIT 1;
                "#,
            )
            .bind(("file_a", source_file.to_owned()))
            .bind(("file_b", target_file.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB get co_change query failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB get co_change decode failed: {err}"))
        })?;
        let mut records = decode_rows::<CouplingEdgeRecord>(rows)?;
        Ok(records.pop())
    }

    pub async fn list_co_change_edges_for_file(
        &self,
        file_path: &str,
        min_fused_score: f32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(Vec::new());
        }
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    file_a: file_a,
                    file_b: file_b,
                    co_change_count: co_change_count,
                    total_commits_a: total_commits_a,
                    total_commits_b: total_commits_b,
                    git_coupling: git_coupling,
                    static_signal: static_signal,
                    semantic_signal: semantic_signal,
                    fused_score: fused_score,
                    coupling_type: coupling_type,
                    last_co_change_commit: last_co_change_commit,
                    last_co_change_at: last_co_change_at,
                    mined_at: mined_at
                }
                FROM co_change
                WHERE (file_a = $file_path OR file_b = $file_path) AND fused_score >= $min_fused_score;
                "#,
            )
            .bind(("file_path", file_path.to_owned()))
            .bind(("min_fused_score", min_fused_score))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB list co_change_edges_for_file failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB list co_change_edges_for_file decode failed: {err}"
            ))
        })?;
        let mut records = decode_rows::<CouplingEdgeRecord>(rows)?;
        records.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        Ok(records)
    }

    pub async fn list_top_co_change_edges(
        &self,
        limit: u32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        let rows: Vec<CouplingEdgeRecord> = self
            .query_rows(
                r#"
                SELECT VALUE {
                    file_a: file_a,
                    file_b: file_b,
                    co_change_count: co_change_count,
                    total_commits_a: total_commits_a,
                    total_commits_b: total_commits_b,
                    git_coupling: git_coupling,
                    static_signal: static_signal,
                    semantic_signal: semantic_signal,
                    fused_score: fused_score,
                    coupling_type: coupling_type,
                    last_co_change_commit: last_co_change_commit,
                    last_co_change_at: last_co_change_at,
                    mined_at: mined_at
                }
                FROM co_change;
                "#,
            )
            .await?;
        let mut records = rows;
        records.sort_by(|left, right| {
            right
                .fused_score
                .partial_cmp(&left.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.file_a.cmp(&right.file_a))
                .then_with(|| left.file_b.cmp(&right.file_b))
        });
        records.truncate(limit.clamp(1, 200) as usize);
        Ok(records)
    }

    pub async fn replace_tested_by_for_test_file(
        &self,
        test_file: &str,
        records: &[TestedByRecord],
    ) -> Result<(), StoreError> {
        let test_file = test_file.trim();
        if test_file.is_empty() {
            return Ok(());
        }

        self.db
            .query("DELETE tested_by WHERE test_file = $test_file;")
            .bind(("test_file", test_file.to_owned()))
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB clear tested_by failed: {err}")))?;

        for record in records {
            self.db
                .query(
                    r#"
                    CREATE tested_by SET
                        target_file = $target_file,
                        test_file = $test_file,
                        intent_count = $intent_count,
                        confidence = $confidence,
                        inference_method = $inference_method;
                    "#,
                )
                .bind(("target_file", record.target_file.clone()))
                .bind(("test_file", record.test_file.clone()))
                .bind(("intent_count", record.intent_count.max(0)))
                .bind(("confidence", record.confidence.clamp(0.0, 1.0)))
                .bind(("inference_method", record.inference_method.clone()))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!("SurrealDB insert tested_by failed: {err}"))
                })?;
        }
        Ok(())
    }

    pub async fn list_tested_by_for_target_file(
        &self,
        target_file: &str,
    ) -> Result<Vec<TestedByRecord>, StoreError> {
        let target_file = target_file.trim();
        if target_file.is_empty() {
            return Ok(Vec::new());
        }
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    target_file: target_file,
                    test_file: test_file,
                    intent_count: intent_count,
                    confidence: confidence,
                    inference_method: inference_method
                }
                FROM tested_by
                WHERE target_file = $target_file;
                "#,
            )
            .bind(("target_file", target_file.to_owned()))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB list tested_by query failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB list tested_by decode failed: {err}"))
        })?;
        let mut records = decode_rows::<TestedByRecord>(rows)?;
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

fn surreal_handle_cache() -> &'static Mutex<HashMap<std::path::PathBuf, Surreal<Db>>> {
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, Surreal<Db>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn schema_init_cache() -> &'static Mutex<HashSet<std::path::PathBuf>> {
    static CACHE: OnceLock<Mutex<HashSet<std::path::PathBuf>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn cached_surreal_handle(graph_dir: &std::path::Path) -> Result<Option<Surreal<Db>>, StoreError> {
    let cache = surreal_handle_cache()
        .lock()
        .map_err(|_| StoreError::Graph("SurrealDB handle cache lock poisoned".to_owned()))?;
    Ok(cache.get(graph_dir).cloned())
}

fn cache_surreal_handle(graph_dir: std::path::PathBuf, db: Surreal<Db>) -> Result<(), StoreError> {
    let mut cache = surreal_handle_cache()
        .lock()
        .map_err(|_| StoreError::Graph("SurrealDB handle cache lock poisoned".to_owned()))?;
    cache.entry(graph_dir).or_insert(db);
    Ok(())
}

fn is_schema_initialized(graph_dir: &std::path::Path) -> Result<bool, StoreError> {
    let cache = schema_init_cache()
        .lock()
        .map_err(|_| StoreError::Graph("SurrealDB schema cache lock poisoned".to_owned()))?;
    Ok(cache.contains(graph_dir))
}

fn mark_schema_initialized(graph_dir: std::path::PathBuf) -> Result<(), StoreError> {
    let mut cache = schema_init_cache()
        .lock()
        .map_err(|_| StoreError::Graph("SurrealDB schema cache lock poisoned".to_owned()))?;
    cache.insert(graph_dir);
    Ok(())
}

#[async_trait]
impl GraphStore for SurrealGraphStore {
    async fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        let symbol_id = symbol.id.clone();
        let qualified_name = symbol.qualified_name.clone();
        let name = symbol_name(symbol.qualified_name.as_str()).to_owned();
        let kind = symbol.kind.clone();
        let file_path = symbol.file_path.clone();
        let language = symbol.language.clone();
        let signature_fingerprint = symbol.signature_fingerprint.clone();
        let last_seen_at = symbol.last_seen_at;
        self.db
            .query(
                r#"
                UPSERT symbol SET
                    symbol_id = $symbol_id,
                    qualified_name = $qualified_name,
                    name = $name,
                    kind = $kind,
                    file_path = $file_path,
                    language = $language,
                    signature_fingerprint = $signature_fingerprint,
                    last_seen_at = $last_seen_at,
                    updated_at = time::now()
                WHERE symbol_id = $symbol_id;
                "#,
            )
            .bind(("symbol_id", symbol_id))
            .bind(("qualified_name", qualified_name))
            .bind(("name", name))
            .bind(("kind", kind))
            .bind(("file_path", file_path))
            .bind(("language", language))
            .bind(("signature_fingerprint", signature_fingerprint))
            .bind(("last_seen_at", last_seen_at))
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB upsert symbol failed: {err}")))?;
        Ok(())
    }

    async fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        let source_id = edge.source_id.clone();
        let target_id = edge.target_id.clone();
        let file_path = edge.file_path.clone();
        let edge_kind = match edge.edge_kind {
            EdgeKind::Calls => "calls",
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::TypeRef => "type_ref",
            EdgeKind::Implements => "implements",
        }
        .to_owned();
        self.db
            .query(
                r#"
                LET $srcs = (SELECT VALUE id FROM symbol WHERE symbol_id = $source_id LIMIT 1);
                LET $dsts = (SELECT VALUE id FROM symbol WHERE symbol_id = $target_id LIMIT 1);
                LET $src = array::first($srcs);
                LET $dst = array::first($dsts);
                IF $src = NONE OR $dst = NONE {
                    RETURN NONE;
                };
                DELETE depends_on WHERE in = $src AND out = $dst AND file_path = $file_path AND edge_kind = $edge_kind;
                RELATE $src->depends_on->$dst SET
                    edge_kind = $edge_kind,
                    file_path = $file_path,
                    source_symbol_id = $source_id,
                    target_symbol_id = $target_id,
                    weight = 1.0;
                "#,
            )
            .bind(("source_id", source_id))
            .bind(("target_id", target_id))
            .bind(("file_path", file_path))
            .bind(("edge_kind", edge_kind))
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB upsert edge failed: {err}")))?;
        Ok(())
    }

    async fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }
        let target_ids: Vec<String> = {
            let mut response = self
                .db
                .query("SELECT VALUE symbol_id FROM symbol WHERE qualified_name = $qualified_name;")
                .bind(("qualified_name", qualified_name.to_owned()))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!("SurrealDB get_callers target lookup failed: {err}"))
                })?;
            response.take(0).map_err(|err| {
                StoreError::Graph(format!("SurrealDB get_callers target decode failed: {err}"))
            })?
        };
        if target_ids.is_empty() {
            return Ok(Vec::new());
        }

        let caller_ids: Vec<String> = {
            let mut response = self
                .db
                .query(
                    r#"
                    SELECT VALUE source_symbol_id
                    FROM depends_on
                    WHERE edge_kind = "calls" AND target_symbol_id INSIDE $target_ids;
                    "#,
                )
                .bind(("target_ids", target_ids))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!(
                        "SurrealDB get_callers caller-id query failed: {err}"
                    ))
                })?;
            response.take(0).map_err(|err| {
                StoreError::Graph(format!(
                    "SurrealDB get_callers caller-id decode failed: {err}"
                ))
            })?
        };
        let mut caller_ids = caller_ids
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        caller_ids.sort();
        caller_ids.dedup();
        if caller_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    id: symbol_id,
                    file_path: file_path,
                    language: language,
                    kind: kind,
                    qualified_name: qualified_name,
                    signature_fingerprint: signature_fingerprint,
                    last_seen_at: last_seen_at
                }
                FROM symbol
                WHERE symbol_id INSIDE $caller_ids;
                "#,
            )
            .bind(("caller_ids", caller_ids))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB get_callers query failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB get_callers decode failed: {err}"))
        })?;
        let mut records = decode_symbol_records(rows)?;
        records.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    async fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }
        let dependency_ids: Vec<String> = {
            let mut response = self
                .db
                .query(
                    r#"
                    SELECT VALUE target_symbol_id
                    FROM depends_on
                    WHERE edge_kind = "calls" AND source_symbol_id = $symbol_id;
                    "#,
                )
                .bind(("symbol_id", symbol_id.to_owned()))
                .await
                .map_err(|err| {
                    StoreError::Graph(format!(
                        "SurrealDB get_dependencies dependency-id query failed: {err}"
                    ))
                })?;
            response.take(0).map_err(|err| {
                StoreError::Graph(format!(
                    "SurrealDB get_dependencies dependency-id decode failed: {err}"
                ))
            })?
        };
        let mut dependency_ids = dependency_ids
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        dependency_ids.sort();
        dependency_ids.dedup();
        if dependency_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    id: symbol_id,
                    file_path: file_path,
                    language: language,
                    kind: kind,
                    qualified_name: qualified_name,
                    signature_fingerprint: signature_fingerprint,
                    last_seen_at: last_seen_at
                }
                FROM symbol
                WHERE symbol_id INSIDE $dependency_ids;
                "#,
            )
            .bind(("dependency_ids", dependency_ids))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB get_dependencies query failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!("SurrealDB get_dependencies decode failed: {err}"))
        })?;
        let mut records = decode_symbol_records(rows)?;
        records.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    async fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        let mut min_depth = HashMap::<String, u32>::new();
        let mut frontier = BTreeSet::<String>::new();
        frontier.insert(symbol_id.to_owned());

        for current_depth in 1..=depth {
            if frontier.is_empty() {
                break;
            }

            let sources = frontier.iter().cloned().collect::<Vec<_>>();
            let edges = self
                .list_dependency_edges_for_sources_by_kind(sources.as_slice(), &["calls"])
                .await?;
            if edges.is_empty() {
                break;
            }

            let mut next = BTreeSet::<String>::new();
            for edge in edges {
                let target = edge.target_id;
                let entry = min_depth.entry(target.clone()).or_insert(current_depth);
                if *entry == current_depth {
                    next.insert(target);
                }
            }
            frontier = next;
        }

        if min_depth.is_empty() {
            return Ok(Vec::new());
        }

        let found_ids: Vec<String> = min_depth.keys().cloned().collect();
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    id: symbol_id,
                    file_path: file_path,
                    language: language,
                    kind: kind,
                    qualified_name: qualified_name,
                    signature_fingerprint: signature_fingerprint,
                    last_seen_at: last_seen_at
                }
                FROM symbol
                WHERE symbol_id INSIDE $found_ids;
                "#,
            )
            .bind(("found_ids", found_ids))
            .await
            .map_err(|err| {
                StoreError::Graph(format!(
                    "SurrealDB get_call_chain symbol fetch failed: {err}"
                ))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB get_call_chain symbol decode failed: {err}"
            ))
        })?;
        let fetched = decode_symbol_records(rows)?;
        let by_id: HashMap<String, SymbolRecord> = fetched
            .into_iter()
            .map(|row| (row.id.clone(), row))
            .collect::<HashMap<_, _>>();

        let mut levels = vec![Vec::<SymbolRecord>::new(); depth as usize];
        for (node_id, node_depth) in min_depth {
            if let Some(record) = by_id.get(node_id.as_str()) {
                let index = node_depth.saturating_sub(1) as usize;
                if index < levels.len() {
                    levels[index].push(record.clone());
                }
            }
        }
        for level in &mut levels {
            level.sort_by(|left, right| {
                left.qualified_name
                    .cmp(&right.qualified_name)
                    .then_with(|| left.id.cmp(&right.id))
            });
        }
        levels.retain(|level| !level.is_empty());
        Ok(levels)
    }

    async fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(());
        }
        let file_path = file_path.to_owned();
        self.db
            .query("DELETE depends_on WHERE file_path = $file_path;")
            .bind(("file_path", file_path))
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB delete edges failed: {err}")))?;
        Ok(())
    }

    async fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        SurrealGraphStore::delete_symbols_batch(self, symbol_ids).await
    }
}

fn symbol_name(qualified_name: &str) -> &str {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
}

fn decode_symbol_records(rows: Vec<serde_json::Value>) -> Result<Vec<SymbolRecord>, StoreError> {
    decode_rows(rows)
}

impl SurrealGraphStore {
    async fn query_rows<T: DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>, StoreError> {
        let mut response = self
            .db
            .query(sql)
            .await
            .map_err(|err| StoreError::Graph(format!("SurrealDB query failed: {err}")))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|err| StoreError::Graph(format!("SurrealDB query decode failed: {err}")))?;
        decode_rows(rows)
    }

    async fn list_dependency_edges_raw(&self) -> Result<Vec<DependencyEdgeRow>, StoreError> {
        self.list_dependency_edges_by_kind(STRUCTURAL_EDGE_KINDS)
            .await
    }

    async fn list_dependency_edges_for_sources_by_kind(
        &self,
        source_ids: &[String],
        edge_kinds: &[&str],
    ) -> Result<Vec<DependencyEdgeRow>, StoreError> {
        if source_ids.is_empty() || edge_kinds.is_empty() {
            return Ok(Vec::new());
        }

        let mut source_ids = source_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        source_ids.sort();
        source_ids.dedup();
        if source_ids.is_empty() {
            return Ok(Vec::new());
        }

        let edge_kinds = edge_kinds
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    source_id: source_symbol_id,
                    target_id: target_symbol_id,
                    edge_kind: edge_kind
                }
                FROM depends_on
                WHERE source_symbol_id INSIDE $source_ids AND edge_kind INSIDE $edge_kinds;
                "#,
            )
            .bind(("source_ids", source_ids))
            .bind(("edge_kinds", edge_kinds))
            .await
            .map_err(|err| {
                StoreError::Graph(format!(
                    "SurrealDB list dependency edges by source failed: {err}"
                ))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB list dependency edges by source decode failed: {err}"
            ))
        })?;
        let mut edges = decode_rows::<DependencyEdgeRow>(rows)?;
        edges.retain(|edge| !edge.source_id.is_empty() && !edge.target_id.is_empty());
        edges.retain(|edge| STRUCTURAL_EDGE_KINDS.contains(&edge.edge_kind.as_str()));
        edges.sort_by(|left, right| {
            left.source_id
                .cmp(&right.source_id)
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.edge_kind.cmp(&right.edge_kind))
        });
        Ok(edges)
    }

    async fn list_dependency_edges_by_kind(
        &self,
        edge_kinds: &[&str],
    ) -> Result<Vec<DependencyEdgeRow>, StoreError> {
        let edge_kinds = edge_kinds
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        let mut response = self
            .db
            .query(
                r#"
                SELECT VALUE {
                    source_id: source_symbol_id,
                    target_id: target_symbol_id,
                    edge_kind: edge_kind
                }
                FROM depends_on
                WHERE edge_kind INSIDE $edge_kinds;
                "#,
            )
            .bind(("edge_kinds", edge_kinds))
            .await
            .map_err(|err| {
                StoreError::Graph(format!("SurrealDB list dependency edges failed: {err}"))
            })?;
        let rows: Vec<serde_json::Value> = response.take(0).map_err(|err| {
            StoreError::Graph(format!(
                "SurrealDB list dependency edges decode failed: {err}"
            ))
        })?;
        let mut edges = decode_rows::<DependencyEdgeRow>(rows)?;
        edges.retain(|edge| !edge.source_id.is_empty() && !edge.target_id.is_empty());
        edges.retain(|edge| STRUCTURAL_EDGE_KINDS.contains(&edge.edge_kind.as_str()));
        edges.sort_by(|left, right| {
            left.source_id
                .cmp(&right.source_id)
                .then_with(|| left.target_id.cmp(&right.target_id))
                .then_with(|| left.edge_kind.cmp(&right.edge_kind))
        });
        Ok(edges)
    }
}

fn decode_rows<T: DeserializeOwned>(rows: Vec<serde_json::Value>) -> Result<Vec<T>, StoreError> {
    rows.into_iter()
        .map(|row| serde_json::from_value(row).map_err(StoreError::Json))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
struct DependencyEdgeRow {
    source_id: String,
    target_id: String,
    edge_kind: String,
}

fn to_algo_edges(edges: Vec<DependencyEdgeRow>) -> Vec<GraphAlgorithmEdge> {
    edges
        .into_iter()
        .map(|edge| GraphAlgorithmEdge {
            source_id: edge.source_id,
            target_id: edge.target_id,
            edge_kind: edge.edge_kind,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn symbol(id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn surreal_graph_supports_concurrent_open_write_read() {
        let temp = tempdir().expect("tempdir");
        let writer = SurrealGraphStore::open(temp.path())
            .await
            .expect("open writer");
        let reader = SurrealGraphStore::open(temp.path())
            .await
            .expect("open reader");

        let alpha = symbol("sym-alpha", "alpha", "src/lib.rs");
        let beta = symbol("sym-beta", "beta", "src/lib.rs");
        writer
            .upsert_symbol_node(&alpha)
            .await
            .expect("upsert alpha");
        writer.upsert_symbol_node(&beta).await.expect("upsert beta");
        writer
            .upsert_edge(&ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            })
            .await
            .expect("upsert edge");

        let deps = reader
            .get_dependencies(&alpha.id)
            .await
            .expect("get dependencies");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].id, beta.id);
    }

    #[tokio::test]
    async fn surreal_graph_call_chain_and_helpers_work() {
        let temp = tempdir().expect("tempdir");
        let graph = SurrealGraphStore::open(temp.path())
            .await
            .expect("open graph");

        let alpha = symbol("sym-alpha", "alpha", "src/a.rs");
        let beta = symbol("sym-beta", "beta", "src/b.rs");
        let gamma = symbol("sym-gamma", "gamma", "src/c.rs");
        for row in [&alpha, &beta, &gamma] {
            graph.upsert_symbol_node(row).await.expect("upsert symbol");
        }
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/a.rs".to_owned(),
            })
            .await
            .expect("edge a->b");
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/b.rs".to_owned(),
            })
            .await
            .expect("edge b->c");

        let chain = graph
            .get_call_chain(&alpha.id, 3)
            .await
            .expect("call chain");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0][0].id, beta.id);

        assert!(
            graph
                .has_dependency_between_files("src/a.rs", "src/b.rs")
                .await
                .expect("has file dep")
        );

        let traversal = graph
            .list_upstream_dependency_traversal(&alpha.id, 3)
            .await
            .expect("traversal");
        assert!(!traversal.edges.is_empty());
    }

    #[tokio::test]
    async fn upsert_edge_deduplicates_on_reindex() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".aether")).expect("mkdir");
        let graph = SurrealGraphStore::open(&workspace).await.expect("open");

        let sym_a = symbol("alpha", "alpha", "src/a.rs");
        let sym_b = symbol("beta", "beta", "src/b.rs");
        graph.upsert_symbol_node(&sym_a).await.expect("upsert a");
        graph.upsert_symbol_node(&sym_b).await.expect("upsert b");

        let edge = ResolvedEdge {
            source_id: "alpha".to_owned(),
            target_id: "beta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/a.rs".to_owned(),
        };

        graph.upsert_edge(&edge).await.expect("upsert 1");
        graph.upsert_edge(&edge).await.expect("upsert 2");
        graph.upsert_edge(&edge).await.expect("upsert 3");

        let edges = graph
            .list_dependency_edges_by_kind(&["calls"])
            .await
            .expect("list edges");
        let matching = edges
            .iter()
            .filter(|e| e.source_id == "alpha" && e.target_id == "beta")
            .count();
        assert_eq!(
            matching, 1,
            "expected exactly one edge after three upserts, got {matching}"
        );
    }

    #[tokio::test]
    async fn surreal_graph_round_trips_new_structural_edge_kinds() {
        let temp = tempdir().expect("tempdir");
        let graph = SurrealGraphStore::open(temp.path())
            .await
            .expect("open graph");

        let alpha = symbol("sym-alpha", "alpha", "src/a.rs");
        let target = symbol("sym-target", "Target", "src/a.rs");
        let store_trait = symbol("sym-store", "Store", "src/a.rs");
        let impl_type = symbol("sym-impl", "SqliteStore", "src/a.rs");
        for row in [&alpha, &target, &store_trait, &impl_type] {
            graph.upsert_symbol_node(row).await.expect("upsert symbol");
        }

        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: target.id.clone(),
                edge_kind: EdgeKind::TypeRef,
                file_path: "src/a.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: impl_type.id.clone(),
                target_id: store_trait.id.clone(),
                edge_kind: EdgeKind::Implements,
                file_path: "src/a.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).await.expect("upsert edge");
        }

        let edges = graph
            .list_dependency_edges()
            .await
            .expect("list dependency edges");
        assert!(edges.iter().any(|edge| {
            edge.source_symbol_id == alpha.id
                && edge.target_symbol_id == target.id
                && edge.edge_kind == "type_ref"
        }));
        assert!(edges.iter().any(|edge| {
            edge.source_symbol_id == impl_type.id
                && edge.target_symbol_id == store_trait.id
                && edge.edge_kind == "implements"
        }));
    }

    #[tokio::test]
    async fn surreal_graph_co_change_and_tested_by_round_trip() {
        let temp = tempdir().expect("tempdir");
        let graph = SurrealGraphStore::open(temp.path())
            .await
            .expect("open graph");

        let coupling = CouplingEdgeRecord {
            file_a: "src/a.rs".to_owned(),
            file_b: "src/b.rs".to_owned(),
            co_change_count: 2,
            total_commits_a: 10,
            total_commits_b: 12,
            git_coupling: 0.3,
            static_signal: 0.4,
            semantic_signal: 0.5,
            fused_score: 0.6,
            coupling_type: "mixed".to_owned(),
            last_co_change_commit: "abc123".to_owned(),
            last_co_change_at: 100,
            mined_at: 200,
        };
        graph
            .upsert_co_change_edges(std::slice::from_ref(&coupling))
            .await
            .expect("upsert co_change");
        let loaded = graph
            .get_co_change_edge("src/a.rs", "src/b.rs")
            .await
            .expect("get co_change")
            .expect("co_change exists");
        assert_eq!(loaded.file_a, coupling.file_a);

        let by_file = graph
            .list_co_change_edges_for_file("src/a.rs", 0.0)
            .await
            .expect("list co_change");
        assert_eq!(by_file.len(), 1);

        let tested = TestedByRecord {
            target_file: "src/payment.rs".to_owned(),
            test_file: "tests/payment.rs".to_owned(),
            intent_count: 3,
            confidence: 0.8,
            inference_method: "heuristic".to_owned(),
        };
        graph
            .replace_tested_by_for_test_file(&tested.test_file, std::slice::from_ref(&tested))
            .await
            .expect("replace tested_by");
        let rows = graph
            .list_tested_by_for_target_file(&tested.target_file)
            .await
            .expect("list tested_by");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].test_file, tested.test_file);
    }

    #[tokio::test]
    async fn delete_symbols_batch_removes_nodes_and_edges() {
        let temp = tempdir().expect("tempdir");
        let graph = SurrealGraphStore::open(temp.path())
            .await
            .expect("open graph");

        let alpha = symbol("sym-alpha", "alpha", "src/a.rs");
        let beta = symbol("sym-beta", "beta", "src/b.rs");
        let gamma = symbol("sym-gamma", "gamma", "src/c.rs");
        for row in [&alpha, &beta, &gamma] {
            graph.upsert_symbol_node(row).await.expect("upsert symbol");
        }
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/a.rs".to_owned(),
            })
            .await
            .expect("edge alpha -> beta");
        graph
            .upsert_edge(&ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: alpha.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/c.rs".to_owned(),
            })
            .await
            .expect("edge gamma -> alpha");

        graph
            .delete_symbols_batch(&[alpha.id.clone(), beta.id.clone()])
            .await
            .expect("delete symbol batch");

        let remaining_ids = graph.list_all_symbol_ids().await.expect("list symbol ids");
        assert_eq!(remaining_ids, vec![gamma.id]);

        let edges = graph
            .list_dependency_edges_by_kind(&["calls"])
            .await
            .expect("list edges");
        assert!(edges.is_empty());
    }
}
