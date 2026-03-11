use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;

use super::{
    CouplingEdgeRecord, GraphStore, ResolvedEdge, StoreError, SurrealGraphStore, SymbolRecord,
    TestedByRecord, UpstreamDependencyTraversal,
};

pub type CrossCommunityEdge = (String, String, String, i64, i64);

pub struct CozoGraphStore {
    inner: SurrealGraphStore,
}

impl CozoGraphStore {
    fn runtime() -> &'static tokio::runtime::Runtime {
        static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("global CozoGraphStore compatibility tokio runtime should initialize")
        })
    }

    fn block_on_runtime<F, T>(future: F) -> T
    where
        F: Future<Output = T> + Send,
        T: Send,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| Self::runtime().block_on(future))
                    .join()
                    .expect("CozoGraphStore compatibility runtime thread should not panic")
            })
        } else {
            Self::runtime().block_on(future)
        }
    }

    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let inner = Self::block_on_runtime(SurrealGraphStore::open(workspace_root))?;
        Ok(Self { inner })
    }

    pub fn open_readonly(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open(workspace_root)
    }

    pub fn list_louvain_communities(&self) -> Result<Vec<(String, i64)>, StoreError> {
        Self::block_on_runtime(self.inner.list_louvain_communities())
    }

    pub fn list_cross_community_edges(
        &self,
        community_by_symbol: &HashMap<String, i64>,
    ) -> Result<Vec<CrossCommunityEdge>, StoreError> {
        Self::block_on_runtime(self.inner.list_cross_community_edges(community_by_symbol))
    }

    pub fn list_pagerank(&self) -> Result<Vec<(String, f32)>, StoreError> {
        Self::block_on_runtime(self.inner.list_pagerank())
    }

    pub fn list_strongly_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        Self::block_on_runtime(self.inner.list_strongly_connected_components())
    }

    pub fn list_connected_components(&self) -> Result<Vec<Vec<String>>, StoreError> {
        Self::block_on_runtime(self.inner.list_connected_components())
    }

    pub fn has_dependency_between_files(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, StoreError> {
        Self::block_on_runtime(self.inner.has_dependency_between_files(file_a, file_b))
    }

    pub fn list_upstream_dependency_traversal(
        &self,
        target_symbol_id: &str,
        max_depth: u32,
    ) -> Result<UpstreamDependencyTraversal, StoreError> {
        Self::block_on_runtime(
            self.inner
                .list_upstream_dependency_traversal(target_symbol_id, max_depth),
        )
    }

    pub fn upsert_co_change_edges(&self, records: &[CouplingEdgeRecord]) -> Result<(), StoreError> {
        Self::block_on_runtime(self.inner.upsert_co_change_edges(records))
    }

    pub fn get_co_change_edge(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<Option<CouplingEdgeRecord>, StoreError> {
        Self::block_on_runtime(self.inner.get_co_change_edge(file_a, file_b))
    }

    pub fn list_co_change_edges_for_file(
        &self,
        file_path: &str,
        min_fused_score: f32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        Self::block_on_runtime(
            self.inner
                .list_co_change_edges_for_file(file_path, min_fused_score),
        )
    }

    pub fn list_top_co_change_edges(
        &self,
        limit: u32,
    ) -> Result<Vec<CouplingEdgeRecord>, StoreError> {
        Self::block_on_runtime(self.inner.list_top_co_change_edges(limit))
    }

    pub fn replace_tested_by_for_test_file(
        &self,
        test_file: &str,
        records: &[TestedByRecord],
    ) -> Result<(), StoreError> {
        Self::block_on_runtime(
            self.inner
                .replace_tested_by_for_test_file(test_file, records),
        )
    }

    pub fn list_tested_by_for_target_file(
        &self,
        target_file: &str,
    ) -> Result<Vec<TestedByRecord>, StoreError> {
        Self::block_on_runtime(self.inner.list_tested_by_for_target_file(target_file))
    }

    pub fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::upsert_symbol_node(self, symbol))
    }

    pub fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::upsert_edge(self, edge))
    }

    pub fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::get_callers(self, qualified_name))
    }

    pub fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::get_dependencies(self, symbol_id))
    }

    pub fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::get_call_chain(self, symbol_id, depth))
    }

    pub fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::delete_edges_for_file(self, file_path))
    }

    pub fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        Self::block_on_runtime(<Self as GraphStore>::delete_symbols_batch(self, symbol_ids))
    }
}

#[async_trait]
impl GraphStore for CozoGraphStore {
    async fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        self.inner.upsert_symbol_node(symbol).await
    }

    async fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        self.inner.upsert_edge(edge).await
    }

    async fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        self.inner.get_callers(qualified_name).await
    }

    async fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        self.inner.get_dependencies(symbol_id).await
    }

    async fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        self.inner.get_call_chain(symbol_id, depth).await
    }

    async fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        self.inner.delete_edges_for_file(file_path).await
    }

    async fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        self.inner.delete_symbols_batch(symbol_ids).await
    }
}
