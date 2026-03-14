use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aether_config::{GraphBackend, load_workspace_config};
use aether_core::{EdgeKind, SymbolEdge, content_hash, normalize_path};
use async_trait::async_trait;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    params_from_iter, types::Value as SqlValue,
};
use serde::{Deserialize, Serialize};
use serde_json::from_str as json_from_str;
use thiserror::Error;
mod analysis;
mod embeddings;
mod graph;
mod lexical;
mod lifecycle;
mod project_notes;
mod schema;
mod sir_history;
mod sir_meta;
mod snapshots;
mod symbols;
mod test_intents;
mod thresholds;
mod write_intents;

pub mod document_store;
pub mod document_vector_store;
mod graph_cozo_compat;
mod graph_sqlite;
mod graph_surreal;
#[cfg(test)]
mod tests;
mod vector;

pub use graph_cozo_compat::CozoGraphStore;
pub use graph_sqlite::SqliteGraphStore;
pub use graph_surreal::SurrealGraphStore;
pub use vector::{
    LanceVectorStore, ProjectNoteVectorRecord, ProjectNoteVectorSearchResult, SqliteVectorStore,
    VectorEmbeddingMetaRecord, VectorRecord, VectorSearchResult, VectorStore, open_vector_store,
};

pub use analysis::{
    CommunitySnapshotRecord, CouplingMiningStateRecord, DriftAnalysisStateRecord, DriftResultRecord,
};
pub use embeddings::{SemanticSearchResult, SymbolEmbeddingMetaRecord, SymbolEmbeddingRecord};
pub use graph::{
    CouplingEdgeRecord, GraphDependencyEdgeRecord, GraphSyncStats, ResolvedEdge, TestedByRecord,
    UpstreamDependencyEdgeRecord, UpstreamDependencyNodeRecord, UpstreamDependencyTraversal,
};
pub use project_notes::{
    ProjectEntityRefRecord, ProjectNoteEmbeddingRecord, ProjectNoteRecord,
    ProjectNoteSemanticSearchResult,
};
pub use schema::SchemaVersion;
pub use sir_history::{
    SirHistoryBaselineSelector, SirHistoryRecord, SirHistoryResolvedPair, SirHistorySelector,
    SirVersionWriteResult,
};
pub use sir_meta::SirMetaRecord;
pub use snapshots::{IntentSnapshot, IntentSnapshotSummary, SnapshotEntry};
pub use symbols::{SymbolMetadata, SymbolRecord, SymbolSearchResult};
pub use test_intents::TestIntentRecord;
pub use thresholds::{CalibrationEmbeddingRecord, ThresholdCalibrationRecord};
pub use write_intents::{IntentOperation, WriteIntent, WriteIntentStatus};

pub(crate) use graph::STRUCTURAL_EDGE_KINDS;
pub(crate) use lexical::project_note_lexical_terms;
pub(crate) use schema::run_migrations;
pub(crate) use sir_history::{
    SirHistoryTransferRecord, append_sir_history_records, load_max_sir_history_version,
    load_sir_history_transfer_records,
};
pub(crate) use sir_meta::{load_sir_row_state, upsert_sir_row_state};
pub(crate) use symbols::{
    load_symbol_access_state, merge_symbol_access_state, update_symbol_access_state,
};

const SYMBOL_ACCESS_COUNTER_MAX: i64 = i64::MAX;
const SYMBOL_ACCESS_DEBOUNCE_SECONDS: u64 = 60;
const SQLITE_PARAM_CHUNK: usize = 900;
const RECONCILE_PARAM_CHUNK: usize = 500;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("config error: {0}")]
    Config(#[from] aether_config::ConfigError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("graph error: {0}")]
    Graph(String),
    #[error("cozo error: {0}")]
    Cozo(String),
    #[error("lancedb error: {0}")]
    LanceDb(String),
    #[error("schema compatibility error: {0}")]
    Compatibility(String),
}

pub trait SymbolCatalogStore {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError>;
    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError>;
    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError>;
    fn increment_symbol_access(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError>;
    fn increment_symbol_access_debounced(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError>;
}

pub trait SymbolRelationStore {
    fn upsert_edges(&self, edges: &[SymbolEdge]) -> Result<(), StoreError>;
    fn get_callers(&self, target_qualified_name: &str) -> Result<Vec<SymbolEdge>, StoreError>;
    fn get_dependencies(&self, source_id: &str) -> Result<Vec<SymbolEdge>, StoreError>;
    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError>;
    fn has_dependency_between_files(&self, file_a: &str, file_b: &str) -> Result<bool, StoreError>;
}

pub trait SirStateStore {
    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError>;
    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError>;

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError>;
    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError>;
}

pub trait SirHistoryStore {
    fn list_sir_history(&self, symbol_id: &str) -> Result<Vec<SirHistoryRecord>, StoreError>;
    fn latest_sir_history_pair(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError>;
    fn resolve_sir_history_pair(
        &self,
        symbol_id: &str,
        from: SirHistorySelector,
        to: SirHistorySelector,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError>;
    #[allow(clippy::too_many_arguments)]
    fn record_sir_version_if_changed(
        &self,
        symbol_id: &str,
        sir_hash: &str,
        provider: &str,
        model: &str,
        sir_json: &str,
        created_at: i64,
        commit_hash: Option<&str>,
    ) -> Result<SirVersionWriteResult, StoreError>;
    fn resolve_sir_baseline_by_selector(
        &self,
        symbol_id: &str,
        selector: SirHistoryBaselineSelector,
    ) -> Result<Option<SirHistoryRecord>, StoreError>;
}

pub trait SnapshotStore {
    fn create_snapshot(&self, snapshot: &IntentSnapshot) -> Result<(), StoreError>;
    fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<IntentSnapshot>, StoreError>;
    fn list_snapshots(&self) -> Result<Vec<IntentSnapshotSummary>, StoreError>;
    fn get_snapshot_entries(&self, snapshot_id: &str) -> Result<Vec<SnapshotEntry>, StoreError>;
    fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), StoreError>;
}

pub trait SemanticIndexStore {
    fn upsert_symbol_embedding(&self, record: SymbolEmbeddingRecord) -> Result<(), StoreError>;
    fn get_symbol_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolEmbeddingMetaRecord>, StoreError>;
    fn delete_symbol_embedding(&self, symbol_id: &str) -> Result<(), StoreError>;
    fn search_symbols_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<SemanticSearchResult>, StoreError>;
    fn list_embeddings_for_provider_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Vec<CalibrationEmbeddingRecord>, StoreError>;
}

pub trait ThresholdStore {
    fn upsert_threshold_calibration(
        &self,
        record: ThresholdCalibrationRecord,
    ) -> Result<(), StoreError>;
    fn get_threshold_calibration(
        &self,
        language: &str,
    ) -> Result<Option<ThresholdCalibrationRecord>, StoreError>;
    fn list_threshold_calibrations(&self) -> Result<Vec<ThresholdCalibrationRecord>, StoreError>;
}

pub trait ProjectNoteStore {
    fn upsert_project_note(&self, record: ProjectNoteRecord) -> Result<(), StoreError>;
    fn find_project_note_by_content_hash(
        &self,
        content_hash: &str,
        include_archived: bool,
    ) -> Result<Option<ProjectNoteRecord>, StoreError>;
    fn get_project_note(&self, note_id: &str) -> Result<Option<ProjectNoteRecord>, StoreError>;
    fn list_project_notes(
        &self,
        limit: u32,
        since_epoch_ms: Option<i64>,
        include_archived: bool,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError>;
    fn list_project_notes_for_file_ref(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError>;
    fn search_project_notes_lexical(
        &self,
        query: &str,
        limit: u32,
        include_archived: bool,
        tags_filter: &[String],
    ) -> Result<Vec<ProjectNoteRecord>, StoreError>;
    fn increment_project_note_access(
        &self,
        note_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError>;
}

pub trait ProjectNoteEmbeddingStore {
    fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteEmbeddingRecord,
    ) -> Result<(), StoreError>;
    fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError>;
    fn search_project_notes_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteSemanticSearchResult>, StoreError>;
}

pub trait CouplingStateStore {
    fn get_coupling_mining_state(&self) -> Result<Option<CouplingMiningStateRecord>, StoreError>;
    fn upsert_coupling_mining_state(
        &self,
        state: CouplingMiningStateRecord,
    ) -> Result<(), StoreError>;
}

pub trait DriftStore {
    fn get_drift_analysis_state(&self) -> Result<Option<DriftAnalysisStateRecord>, StoreError>;
    fn upsert_drift_analysis_state(
        &self,
        state: DriftAnalysisStateRecord,
    ) -> Result<(), StoreError>;
    fn upsert_drift_results(&self, records: &[DriftResultRecord]) -> Result<(), StoreError>;
    fn list_drift_results(
        &self,
        include_acknowledged: bool,
    ) -> Result<Vec<DriftResultRecord>, StoreError>;
    fn list_drift_results_by_ids(
        &self,
        result_ids: &[String],
    ) -> Result<Vec<DriftResultRecord>, StoreError>;
    fn acknowledge_drift_results(&self, result_ids: &[String]) -> Result<u32, StoreError>;
    fn replace_community_snapshot(
        &self,
        snapshot_id: &str,
        captured_at: i64,
        assignments: &[CommunitySnapshotRecord],
    ) -> Result<(), StoreError>;
    fn list_latest_community_snapshot(&self) -> Result<Vec<CommunitySnapshotRecord>, StoreError>;
}

pub trait TestIntentStore {
    fn replace_test_intents_for_file(
        &self,
        file_path: &str,
        intents: &[TestIntentRecord],
    ) -> Result<(), StoreError>;
    fn list_test_intents_for_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError>;
    fn list_test_intents_for_symbol(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError>;
    fn search_test_intents_lexical(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<TestIntentRecord>, StoreError>;
}

pub trait Store:
    SymbolCatalogStore
    + SymbolRelationStore
    + SirStateStore
    + SirHistoryStore
    + SnapshotStore
    + SemanticIndexStore
    + ThresholdStore
    + ProjectNoteStore
    + ProjectNoteEmbeddingStore
    + CouplingStateStore
    + DriftStore
    + TestIntentStore
{
}

impl<T> Store for T where
    T: SymbolCatalogStore
        + SymbolRelationStore
        + SirStateStore
        + SirHistoryStore
        + SnapshotStore
        + SemanticIndexStore
        + ThresholdStore
        + ProjectNoteStore
        + ProjectNoteEmbeddingStore
        + CouplingStateStore
        + DriftStore
        + TestIntentStore
{
}

#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError>;
    async fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError>;
    async fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    async fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    async fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError>;
    async fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError>;
    async fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError>;
}

pub async fn open_graph_store(
    workspace_root: impl AsRef<Path>,
) -> Result<Box<dyn GraphStore>, StoreError> {
    let workspace_root = workspace_root.as_ref();
    let config = load_workspace_config(workspace_root)?;
    match config.storage.graph_backend {
        GraphBackend::Surreal => Ok(Box::new(SurrealGraphStore::open(workspace_root).await?)),
        GraphBackend::Cozo => Err(StoreError::Graph(
            "CozoDB backend removed; run `aether graph-migrate` to convert to SurrealDB".to_owned(),
        )),
        GraphBackend::Sqlite => Ok(Box::new(SqliteGraphStore::open(workspace_root)?)),
    }
}

pub fn open_graph_store_readonly(
    workspace_root: impl AsRef<Path>,
) -> Result<Box<dyn GraphStore>, StoreError> {
    let workspace_root = workspace_root.as_ref();
    let config = load_workspace_config(workspace_root)?;
    match config.storage.graph_backend {
        GraphBackend::Surreal => {
            // Uses CozoGraphStore (sync compat shim) because this function is sync.
            // The shim wraps SurrealGraphStore internally.
            Ok(Box::new(CozoGraphStore::open_readonly(workspace_root)?))
        }
        GraphBackend::Cozo => Ok(Box::new(CozoGraphStore::open_readonly(workspace_root)?)),
        GraphBackend::Sqlite => Ok(Box::new(SqliteGraphStore::open_readonly(workspace_root)?)),
    }
}

pub struct SqliteStore {
    conn: Mutex<Connection>,
    aether_dir: PathBuf,
    sir_dir: PathBuf,
    mirror_sir_files: bool,
    symbol_access_debounce: Mutex<HashMap<String, Instant>>,
}

impl SqliteStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_flags(
            workspace_root,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            false,
        )
    }
    pub fn open_readonly(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_flags(workspace_root, OpenFlags::SQLITE_OPEN_READ_ONLY, true)
    }
    fn open_with_flags(
        workspace_root: impl AsRef<Path>,
        flags: OpenFlags,
        read_only: bool,
    ) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let config = load_workspace_config(workspace_root)?;
        let aether_dir = workspace_root.join(".aether");
        let sir_dir = aether_dir.join("sir");
        let sqlite_path = aether_dir.join("meta.sqlite");

        if !read_only {
            fs::create_dir_all(&sir_dir)?;
        }

        let conn = Connection::open_with_flags(sqlite_path, flags)?;
        if !read_only {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        conn.busy_timeout(Duration::from_secs(5))?;
        if !read_only {
            run_migrations(&conn)?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
            aether_dir,
            sir_dir,
            mirror_sir_files: config.storage.mirror_sir_files,
            symbol_access_debounce: Mutex::new(HashMap::new()),
        })
    }
    pub fn aether_dir(&self) -> &Path {
        &self.aether_dir
    }
    pub fn sir_dir(&self) -> &Path {
        &self.sir_dir
    }
    pub fn workspace_root(&self) -> Option<PathBuf> {
        self.aether_dir.parent().map(Path::to_path_buf)
    }
    pub fn mirror_sir_files_enabled(&self) -> bool {
        self.mirror_sir_files
    }
}

impl SymbolCatalogStore for SqliteStore {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError> {
        self.store_upsert_symbol(record)
    }

    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.store_mark_removed(symbol_id)
    }

    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        self.store_list_symbols_for_file(file_path)
    }

    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError> {
        self.store_search_symbols(query, limit)
    }

    fn increment_symbol_access(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        self.store_increment_symbol_access(symbol_ids, accessed_at)
    }

    fn increment_symbol_access_debounced(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        self.store_increment_symbol_access_debounced(symbol_ids, accessed_at)
    }
}

impl SymbolRelationStore for SqliteStore {
    fn upsert_edges(&self, edges: &[SymbolEdge]) -> Result<(), StoreError> {
        self.store_upsert_edges(edges)
    }

    fn get_callers(&self, target_qualified_name: &str) -> Result<Vec<SymbolEdge>, StoreError> {
        self.store_get_callers(target_qualified_name)
    }

    fn get_dependencies(&self, source_id: &str) -> Result<Vec<SymbolEdge>, StoreError> {
        self.store_get_dependencies(source_id)
    }

    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        self.store_delete_edges_for_file(file_path)
    }

    fn has_dependency_between_files(&self, file_a: &str, file_b: &str) -> Result<bool, StoreError> {
        self.store_has_dependency_between_files(file_a, file_b)
    }
}

impl SirStateStore for SqliteStore {
    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError> {
        self.store_write_sir_blob(symbol_id, sir_json_string)
    }

    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        self.store_read_sir_blob(symbol_id)
    }

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        self.store_upsert_sir_meta(record)
    }

    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError> {
        self.store_get_sir_meta(symbol_id)
    }
}

impl SirHistoryStore for SqliteStore {
    fn list_sir_history(&self, symbol_id: &str) -> Result<Vec<SirHistoryRecord>, StoreError> {
        self.store_list_sir_history(symbol_id)
    }

    fn latest_sir_history_pair(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        self.store_latest_sir_history_pair(symbol_id)
    }

    fn resolve_sir_history_pair(
        &self,
        symbol_id: &str,
        from: SirHistorySelector,
        to: SirHistorySelector,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        self.store_resolve_sir_history_pair(symbol_id, from, to)
    }

    #[allow(clippy::too_many_arguments)]
    fn record_sir_version_if_changed(
        &self,
        symbol_id: &str,
        sir_hash: &str,
        provider: &str,
        model: &str,
        sir_json: &str,
        created_at: i64,
        commit_hash: Option<&str>,
    ) -> Result<SirVersionWriteResult, StoreError> {
        self.store_record_sir_version_if_changed(
            symbol_id,
            sir_hash,
            provider,
            model,
            sir_json,
            created_at,
            commit_hash,
        )
    }

    fn resolve_sir_baseline_by_selector(
        &self,
        symbol_id: &str,
        selector: SirHistoryBaselineSelector,
    ) -> Result<Option<SirHistoryRecord>, StoreError> {
        self.store_resolve_sir_baseline_by_selector(symbol_id, selector)
    }
}

impl SnapshotStore for SqliteStore {
    fn create_snapshot(&self, snapshot: &IntentSnapshot) -> Result<(), StoreError> {
        self.store_create_snapshot(snapshot)
    }

    fn get_snapshot(&self, snapshot_id: &str) -> Result<Option<IntentSnapshot>, StoreError> {
        self.store_get_snapshot(snapshot_id)
    }

    fn list_snapshots(&self) -> Result<Vec<IntentSnapshotSummary>, StoreError> {
        self.store_list_snapshots()
    }

    fn get_snapshot_entries(&self, snapshot_id: &str) -> Result<Vec<SnapshotEntry>, StoreError> {
        self.store_get_snapshot_entries(snapshot_id)
    }

    fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), StoreError> {
        self.store_delete_snapshot(snapshot_id)
    }
}

impl SemanticIndexStore for SqliteStore {
    fn upsert_symbol_embedding(&self, record: SymbolEmbeddingRecord) -> Result<(), StoreError> {
        self.store_upsert_symbol_embedding(record)
    }

    fn get_symbol_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolEmbeddingMetaRecord>, StoreError> {
        self.store_get_symbol_embedding_meta(symbol_id)
    }

    fn delete_symbol_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.store_delete_symbol_embedding(symbol_id)
    }

    fn search_symbols_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<SemanticSearchResult>, StoreError> {
        self.store_search_symbols_semantic(query_embedding, provider, model, limit)
    }

    fn list_embeddings_for_provider_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Vec<CalibrationEmbeddingRecord>, StoreError> {
        self.store_list_embeddings_for_provider_model(provider, model)
    }
}

impl ThresholdStore for SqliteStore {
    fn upsert_threshold_calibration(
        &self,
        record: ThresholdCalibrationRecord,
    ) -> Result<(), StoreError> {
        self.store_upsert_threshold_calibration(record)
    }

    fn get_threshold_calibration(
        &self,
        language: &str,
    ) -> Result<Option<ThresholdCalibrationRecord>, StoreError> {
        self.store_get_threshold_calibration(language)
    }

    fn list_threshold_calibrations(&self) -> Result<Vec<ThresholdCalibrationRecord>, StoreError> {
        self.store_list_threshold_calibrations()
    }
}

impl ProjectNoteStore for SqliteStore {
    fn upsert_project_note(&self, record: ProjectNoteRecord) -> Result<(), StoreError> {
        self.store_upsert_project_note(record)
    }

    fn find_project_note_by_content_hash(
        &self,
        content_hash: &str,
        include_archived: bool,
    ) -> Result<Option<ProjectNoteRecord>, StoreError> {
        self.store_find_project_note_by_content_hash(content_hash, include_archived)
    }

    fn get_project_note(&self, note_id: &str) -> Result<Option<ProjectNoteRecord>, StoreError> {
        self.store_get_project_note(note_id)
    }

    fn list_project_notes(
        &self,
        limit: u32,
        since_epoch_ms: Option<i64>,
        include_archived: bool,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        self.store_list_project_notes(limit, since_epoch_ms, include_archived)
    }

    fn list_project_notes_for_file_ref(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        self.store_list_project_notes_for_file_ref(file_path, limit)
    }

    fn search_project_notes_lexical(
        &self,
        query: &str,
        limit: u32,
        include_archived: bool,
        tags_filter: &[String],
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        self.store_search_project_notes_lexical(query, limit, include_archived, tags_filter)
    }

    fn increment_project_note_access(
        &self,
        note_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        self.store_increment_project_note_access(note_ids, accessed_at)
    }
}

impl ProjectNoteEmbeddingStore for SqliteStore {
    fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteEmbeddingRecord,
    ) -> Result<(), StoreError> {
        self.store_upsert_project_note_embedding(record)
    }

    fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError> {
        self.store_delete_project_note_embedding(note_id)
    }

    fn search_project_notes_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteSemanticSearchResult>, StoreError> {
        self.store_search_project_notes_semantic(query_embedding, provider, model, limit)
    }
}

impl CouplingStateStore for SqliteStore {
    fn get_coupling_mining_state(&self) -> Result<Option<CouplingMiningStateRecord>, StoreError> {
        self.store_get_coupling_mining_state()
    }

    fn upsert_coupling_mining_state(
        &self,
        state: CouplingMiningStateRecord,
    ) -> Result<(), StoreError> {
        self.store_upsert_coupling_mining_state(state)
    }
}

impl DriftStore for SqliteStore {
    fn get_drift_analysis_state(&self) -> Result<Option<DriftAnalysisStateRecord>, StoreError> {
        self.store_get_drift_analysis_state()
    }

    fn upsert_drift_analysis_state(
        &self,
        state: DriftAnalysisStateRecord,
    ) -> Result<(), StoreError> {
        self.store_upsert_drift_analysis_state(state)
    }

    fn upsert_drift_results(&self, records: &[DriftResultRecord]) -> Result<(), StoreError> {
        self.store_upsert_drift_results(records)
    }

    fn list_drift_results(
        &self,
        include_acknowledged: bool,
    ) -> Result<Vec<DriftResultRecord>, StoreError> {
        self.store_list_drift_results(include_acknowledged)
    }

    fn list_drift_results_by_ids(
        &self,
        result_ids: &[String],
    ) -> Result<Vec<DriftResultRecord>, StoreError> {
        self.store_list_drift_results_by_ids(result_ids)
    }

    fn acknowledge_drift_results(&self, result_ids: &[String]) -> Result<u32, StoreError> {
        self.store_acknowledge_drift_results(result_ids)
    }

    fn replace_community_snapshot(
        &self,
        snapshot_id: &str,
        captured_at: i64,
        assignments: &[CommunitySnapshotRecord],
    ) -> Result<(), StoreError> {
        self.store_replace_community_snapshot(snapshot_id, captured_at, assignments)
    }

    fn list_latest_community_snapshot(&self) -> Result<Vec<CommunitySnapshotRecord>, StoreError> {
        self.store_list_latest_community_snapshot()
    }
}

impl TestIntentStore for SqliteStore {
    fn replace_test_intents_for_file(
        &self,
        file_path: &str,
        intents: &[TestIntentRecord],
    ) -> Result<(), StoreError> {
        self.store_replace_test_intents_for_file(file_path, intents)
    }

    fn list_test_intents_for_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        self.store_list_test_intents_for_file(file_path)
    }

    fn list_test_intents_for_symbol(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        self.store_list_test_intents_for_symbol(symbol_id)
    }

    fn search_test_intents_lexical(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        self.store_search_test_intents_lexical(query, limit)
    }
}
