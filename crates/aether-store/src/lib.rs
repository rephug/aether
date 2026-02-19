use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aether_config::{GraphBackend, load_workspace_config};
use aether_core::{EdgeKind, SymbolEdge, normalize_path};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde::{Deserialize, Serialize};
use serde_json::from_str as json_from_str;
use thiserror::Error;

mod graph_cozo;
mod graph_sqlite;
mod vector;
pub use graph_cozo::CozoGraphStore;
pub use graph_sqlite::SqliteGraphStore;
pub use vector::{
    LanceVectorStore, ProjectNoteVectorRecord, ProjectNoteVectorSearchResult, SqliteVectorStore,
    VectorEmbeddingMetaRecord, VectorRecord, VectorSearchResult, VectorStore, open_vector_store,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRecord {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub qualified_name: String,
    pub signature_fingerprint: String,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirMetaRecord {
    pub id: String,
    pub sir_hash: String,
    pub sir_version: i64,
    pub provider: String,
    pub model: String,
    pub updated_at: i64,
    pub sir_status: String,
    pub last_error: Option<String>,
    pub last_attempt_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirHistoryRecord {
    pub symbol_id: String,
    pub version: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub created_at: i64,
    pub sir_json: String,
    pub commit_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SirHistorySelector {
    Version(i64),
    CreatedAt(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirHistoryResolvedPair {
    pub from: SirHistoryRecord,
    pub to: SirHistoryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirVersionWriteResult {
    pub version: i64,
    pub updated_at: i64,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSearchResult {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolEmbeddingRecord {
    pub symbol_id: String,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEmbeddingMetaRecord {
    pub symbol_id: String,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub embedding_dim: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticSearchResult {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntityRefRecord {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNoteRecord {
    pub note_id: String,
    pub content: String,
    pub content_hash: String,
    pub source_type: String,
    pub source_agent: Option<String>,
    pub tags: Vec<String>,
    pub entity_refs: Vec<ProjectEntityRefRecord>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
    pub is_archived: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteEmbeddingRecord {
    pub note_id: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteSemanticSearchResult {
    pub note_id: String,
    pub semantic_score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouplingMiningStateRecord {
    pub last_commit_hash: Option<String>,
    pub last_mined_at: Option<i64>,
    pub commits_scanned: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CouplingEdgeRecord {
    pub file_a: String,
    pub file_b: String,
    pub co_change_count: i64,
    pub total_commits_a: i64,
    pub total_commits_b: i64,
    pub git_coupling: f32,
    pub static_signal: f32,
    pub semantic_signal: f32,
    pub fused_score: f32,
    pub coupling_type: String,
    pub last_co_change_commit: String,
    pub last_co_change_at: i64,
    pub mined_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdCalibrationRecord {
    pub language: String,
    pub threshold: f32,
    pub sample_size: i64,
    pub provider: String,
    pub model: String,
    pub calibrated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationEmbeddingRecord {
    pub symbol_id: String,
    pub file_path: String,
    pub language: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_kind: EdgeKind,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSyncStats {
    pub resolved_edges: usize,
    pub unresolved_edges: usize,
}

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
    #[error("cozo error: {0}")]
    Cozo(String),
    #[error("lancedb error: {0}")]
    LanceDb(String),
}

pub trait Store {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError>;
    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError>;
    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError>;
    fn upsert_edges(&self, edges: &[SymbolEdge]) -> Result<(), StoreError>;
    fn get_callers(&self, target_qualified_name: &str) -> Result<Vec<SymbolEdge>, StoreError>;
    fn get_dependencies(&self, source_id: &str) -> Result<Vec<SymbolEdge>, StoreError>;
    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError>;

    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError>;
    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError>;

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError>;
    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError>;
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
    fn upsert_threshold_calibration(
        &self,
        record: ThresholdCalibrationRecord,
    ) -> Result<(), StoreError>;
    fn get_threshold_calibration(
        &self,
        language: &str,
    ) -> Result<Option<ThresholdCalibrationRecord>, StoreError>;
    fn list_threshold_calibrations(&self) -> Result<Vec<ThresholdCalibrationRecord>, StoreError>;
    fn list_embeddings_for_provider_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Vec<CalibrationEmbeddingRecord>, StoreError>;

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

    fn get_coupling_mining_state(&self) -> Result<Option<CouplingMiningStateRecord>, StoreError>;
    fn upsert_coupling_mining_state(
        &self,
        state: CouplingMiningStateRecord,
    ) -> Result<(), StoreError>;
    fn has_dependency_between_files(&self, file_a: &str, file_b: &str) -> Result<bool, StoreError>;
}

pub trait GraphStore: Send + Sync {
    fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError>;
    fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError>;
    fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError>;
    fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError>;
    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError>;
}

pub fn open_graph_store(
    workspace_root: impl AsRef<Path>,
) -> Result<Box<dyn GraphStore>, StoreError> {
    let workspace_root = workspace_root.as_ref();
    let config = load_workspace_config(workspace_root)?;
    match config.storage.graph_backend {
        GraphBackend::Cozo => Ok(Box::new(CozoGraphStore::open(workspace_root)?)),
        GraphBackend::Sqlite => Ok(Box::new(SqliteGraphStore::open(workspace_root)?)),
    }
}

pub struct SqliteStore {
    conn: Connection,
    aether_dir: PathBuf,
    sir_dir: PathBuf,
    mirror_sir_files: bool,
}

impl SqliteStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let config = load_workspace_config(workspace_root)?;
        let aether_dir = workspace_root.join(".aether");
        let sir_dir = aether_dir.join("sir");
        let sqlite_path = aether_dir.join("meta.sqlite");

        fs::create_dir_all(&sir_dir)?;

        let conn = Connection::open(sqlite_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        run_migrations(&conn)?;

        Ok(Self {
            conn,
            aether_dir,
            sir_dir,
            mirror_sir_files: config.storage.mirror_sir_files,
        })
    }

    pub fn aether_dir(&self) -> &Path {
        &self.aether_dir
    }

    pub fn sir_dir(&self) -> &Path {
        &self.sir_dir
    }

    pub fn mirror_sir_files_enabled(&self) -> bool {
        self.mirror_sir_files
    }

    fn sir_blob_path(&self, symbol_id: &str) -> PathBuf {
        self.sir_dir.join(format!("{symbol_id}.json"))
    }

    fn upsert_sir_json_only(
        &self,
        symbol_id: &str,
        sir_json_string: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json)
            VALUES (?1, '', 1, '', '', unixepoch(), ?2)
            ON CONFLICT(id) DO UPDATE SET
                sir_json = excluded.sir_json
            "#,
            params![symbol_id, sir_json_string],
        )?;

        Ok(())
    }

    fn read_sir_json_from_db(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT sir_json
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let json = stmt
            .query_row(params![symbol_id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
            .filter(|value| !value.trim().is_empty());

        Ok(json)
    }

    pub fn get_symbol_search_result(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolSearchResult>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind
            FROM symbols
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolSearchResult {
                    symbol_id: row.get(0)?,
                    qualified_name: row.get(1)?,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    kind: row.get(4)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    pub fn get_symbol_record(&self, symbol_id: &str) -> Result<Option<SymbolRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    qualified_name: row.get(4)?,
                    signature_fingerprint: row.get(5)?,
                    last_seen_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    pub fn get_symbol_by_qualified_name(
        &self,
        qualified_name: &str,
    ) -> Result<Option<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(None);
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE qualified_name = ?1
            ORDER BY id ASC
            LIMIT 1
            "#,
        )?;

        let record = stmt
            .query_row(params![qualified_name], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    qualified_name: row.get(4)?,
                    signature_fingerprint: row.get(5)?,
                    last_seen_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    pub fn list_module_file_paths(
        &self,
        module_path: &str,
        language: &str,
    ) -> Result<Vec<String>, StoreError> {
        let module_path = normalize_path(module_path.trim().trim_end_matches('/'));
        let language = language.trim();
        if module_path.is_empty() || language.is_empty() {
            return Ok(Vec::new());
        }

        let like_pattern = format!("{module_path}/%");
        let mut stmt = self.conn.prepare(
            r#"
            SELECT DISTINCT file_path
            FROM symbols
            WHERE language = ?1
              AND (file_path = ?2 OR file_path LIKE ?3)
            ORDER BY file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![language, module_path, like_pattern], |row| {
            row.get::<_, String>(0)
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn list_symbol_embeddings_for_ids(
        &self,
        provider: &str,
        model: &str,
        symbol_ids: &[String],
    ) -> Result<Vec<SymbolEmbeddingRecord>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() || symbol_ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = std::iter::repeat_n("?", symbol_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"
            SELECT symbol_id, sir_hash, provider, model, embedding_json, updated_at
            FROM sir_embeddings
            WHERE provider = ?1
              AND model = ?2
              AND symbol_id IN ({placeholders})
            ORDER BY symbol_id ASC
            "#
        );

        let mut params_vec: Vec<SqlValue> = vec![
            SqlValue::Text(provider.to_owned()),
            SqlValue::Text(model.to_owned()),
        ];
        params_vec.extend(symbol_ids.iter().cloned().map(SqlValue::Text));

        let mut stmt = self.conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in rows {
            let (symbol_id, sir_hash, provider, model, embedding_json, updated_at) = row?;
            let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
            if embedding.is_empty() {
                continue;
            }
            records.push(SymbolEmbeddingRecord {
                symbol_id,
                sir_hash,
                provider,
                model,
                embedding,
                updated_at,
            });
        }

        Ok(records)
    }

    pub fn sync_graph_for_file(
        &self,
        graph_store: &dyn GraphStore,
        file_path: &str,
    ) -> Result<GraphSyncStats, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(GraphSyncStats {
                resolved_edges: 0,
                unresolved_edges: 0,
            });
        }

        graph_store.delete_edges_for_file(file_path)?;

        let symbols = self.list_symbols_for_file(file_path)?;
        for symbol in &symbols {
            graph_store.upsert_symbol_node(symbol)?;
        }

        let mut unresolved_edges = 0usize;
        let mut unresolved_stmt = self.conn.prepare(
            r#"
            SELECT e.source_id, e.target_qualified_name
            FROM symbol_edges e
            LEFT JOIN symbols s ON s.qualified_name = e.target_qualified_name
            WHERE e.file_path = ?1
              AND e.edge_kind = 'calls'
              AND s.id IS NULL
            ORDER BY e.source_id ASC, e.target_qualified_name ASC
            "#,
        )?;
        let unresolved_rows = unresolved_stmt.query_map(params![file_path], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in unresolved_rows {
            let (source_id, target_qualified_name) = row?;
            unresolved_edges += 1;
            tracing::debug!(
                source_id = %source_id,
                target_qualified_name = %target_qualified_name,
                file_path = %file_path,
                "unresolved call edge skipped during graph sync"
            );
        }

        let mut resolved_stmt = self.conn.prepare(
            r#"
            SELECT e.source_id, s.id, e.file_path
            FROM symbol_edges e
            JOIN symbols s ON s.qualified_name = e.target_qualified_name
            WHERE e.file_path = ?1
              AND e.edge_kind = 'calls'
            ORDER BY e.source_id ASC, s.id ASC
            "#,
        )?;
        let resolved_rows = resolved_stmt.query_map(params![file_path], |row| {
            Ok(ResolvedEdge {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                edge_kind: EdgeKind::Calls,
                file_path: row.get(2)?,
            })
        })?;

        let mut resolved_edges = 0usize;
        for edge in resolved_rows {
            resolved_edges += 1;
            graph_store.upsert_edge(&edge?)?;
        }

        Ok(GraphSyncStats {
            resolved_edges,
            unresolved_edges,
        })
    }
}

impl Store for SqliteStore {
    fn upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO symbols (
                id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                file_path = excluded.file_path,
                language = excluded.language,
                kind = excluded.kind,
                qualified_name = excluded.qualified_name,
                signature_fingerprint = excluded.signature_fingerprint,
                last_seen_at = excluded.last_seen_at
            "#,
            params![
                record.id,
                record.file_path,
                record.language,
                record.kind,
                record.qualified_name,
                record.signature_fingerprint,
                record.last_seen_at,
            ],
        )?;

        Ok(())
    }

    fn mark_removed(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM sir_embeddings WHERE symbol_id = ?1",
            params![symbol_id],
        )?;
        self.conn.execute(
            "DELETE FROM sir_history WHERE symbol_id = ?1",
            params![symbol_id],
        )?;
        self.conn
            .execute("DELETE FROM symbols WHERE id = ?1", params![symbol_id])?;
        self.conn
            .execute("DELETE FROM sir WHERE id = ?1", params![symbol_id])?;

        let path = self.sir_blob_path(symbol_id);
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        Ok(())
    }

    fn list_symbols_for_file(&self, file_path: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE file_path = ?1
            ORDER BY id
            "#,
        )?;

        let rows = stmt.query_map(params![file_path], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                qualified_name: row.get(4)?,
                signature_fingerprint: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let capped_limit = limit.clamp(1, 100) as i64;
        let pattern = format!("%{query}%");

        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind
            FROM symbols
            WHERE LOWER(id) LIKE LOWER(?1)
               OR LOWER(qualified_name) LIKE LOWER(?1)
               OR LOWER(file_path) LIKE LOWER(?1)
               OR LOWER(language) LIKE LOWER(?1)
               OR LOWER(kind) LIKE LOWER(?1)
            ORDER BY qualified_name ASC, id ASC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![pattern, capped_limit], |row| {
            Ok(SymbolSearchResult {
                symbol_id: row.get(0)?,
                qualified_name: row.get(1)?,
                file_path: row.get(2)?,
                language: row.get(3)?,
                kind: row.get(4)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn upsert_edges(&self, edges: &[SymbolEdge]) -> Result<(), StoreError> {
        if edges.is_empty() {
            return Ok(());
        }

        self.conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;

        let result = (|| -> Result<(), StoreError> {
            let mut stmt = self.conn.prepare(
                r#"
                INSERT INTO symbol_edges (
                    source_id, target_qualified_name, edge_kind, file_path
                )
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(source_id, target_qualified_name, edge_kind) DO UPDATE SET
                    file_path = excluded.file_path
                "#,
            )?;

            for edge in edges {
                stmt.execute(params![
                    edge.source_id,
                    edge.target_qualified_name,
                    edge.edge_kind.as_str(),
                    edge.file_path,
                ])?;
            }

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn get_callers(&self, target_qualified_name: &str) -> Result<Vec<SymbolEdge>, StoreError> {
        let target_qualified_name = target_qualified_name.trim();
        if target_qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT source_id, target_qualified_name, file_path
            FROM symbol_edges
            WHERE edge_kind = 'calls'
              AND target_qualified_name = ?1
            ORDER BY source_id ASC, target_qualified_name ASC, file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![target_qualified_name], |row| {
            Ok(SymbolEdge {
                source_id: row.get(0)?,
                target_qualified_name: row.get(1)?,
                edge_kind: EdgeKind::Calls,
                file_path: row.get(2)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn get_dependencies(&self, source_id: &str) -> Result<Vec<SymbolEdge>, StoreError> {
        let source_id = source_id.trim();
        if source_id.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT source_id, target_qualified_name, file_path
            FROM symbol_edges
            WHERE edge_kind = 'depends_on'
              AND source_id = ?1
            ORDER BY source_id ASC, target_qualified_name ASC, file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![source_id], |row| {
            Ok(SymbolEdge {
                source_id: row.get(0)?,
                target_qualified_name: row.get(1)?,
                edge_kind: EdgeKind::DependsOn,
                file_path: row.get(2)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM symbol_edges WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }

    fn write_sir_blob(&self, symbol_id: &str, sir_json_string: &str) -> Result<(), StoreError> {
        self.upsert_sir_json_only(symbol_id, sir_json_string)?;

        if self.mirror_sir_files {
            let path = self.sir_blob_path(symbol_id);
            if let Err(err) = fs::write(path, sir_json_string) {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "aether-store mirror write failed"
                );
            }
        }

        Ok(())
    }

    fn read_sir_blob(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        if let Some(json) = self.read_sir_json_from_db(symbol_id)? {
            return Ok(Some(json));
        }

        let path = self.sir_blob_path(symbol_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;
        self.upsert_sir_json_only(symbol_id, &content)?;
        Ok(Some(content))
    }

    fn upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO sir (
                id, sir_hash, sir_version, provider, model, updated_at,
                sir_status, last_error, last_attempt_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                sir_version = excluded.sir_version,
                provider = excluded.provider,
                model = excluded.model,
                updated_at = excluded.updated_at,
                sir_status = excluded.sir_status,
                last_error = excluded.last_error,
                last_attempt_at = excluded.last_attempt_at
            "#,
            params![
                record.id,
                record.sir_hash,
                record.sir_version,
                record.provider,
                record.model,
                record.updated_at,
                record.sir_status,
                record.last_error,
                record.last_attempt_at,
            ],
        )?;

        Ok(())
    }

    fn get_sir_meta(&self, symbol_id: &str) -> Result<Option<SirMetaRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                id,
                sir_hash,
                sir_version,
                provider,
                model,
                updated_at,
                sir_status,
                last_error,
                last_attempt_at
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SirMetaRecord {
                    id: row.get(0)?,
                    sir_hash: row.get(1)?,
                    sir_version: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    updated_at: row.get(5)?,
                    sir_status: row.get(6)?,
                    last_error: row.get(7)?,
                    last_attempt_at: row.get(8)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    fn list_sir_history(&self, symbol_id: &str) -> Result<Vec<SirHistoryRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
            FROM sir_history
            WHERE symbol_id = ?1
            ORDER BY version ASC
            "#,
        )?;

        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(SirHistoryRecord {
                symbol_id: row.get(0)?,
                version: row.get(1)?,
                sir_hash: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                created_at: row.get(5)?,
                sir_json: row.get(6)?,
                commit_hash: row.get(7)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn latest_sir_history_pair(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        let history = self.list_sir_history(symbol_id)?;
        let Some(latest) = history.last().cloned() else {
            return Ok(None);
        };
        let from = history
            .get(history.len().saturating_sub(2))
            .cloned()
            .unwrap_or_else(|| latest.clone());

        Ok(Some(SirHistoryResolvedPair { from, to: latest }))
    }

    fn resolve_sir_history_pair(
        &self,
        symbol_id: &str,
        from: SirHistorySelector,
        to: SirHistorySelector,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        let history = self.list_sir_history(symbol_id)?;
        let from_idx = resolve_history_selector_index(&history, &from);
        let to_idx = resolve_history_selector_index(&history, &to);

        let (Some(from_idx), Some(to_idx)) = (from_idx, to_idx) else {
            return Ok(None);
        };
        if from_idx > to_idx {
            return Ok(None);
        }

        Ok(Some(SirHistoryResolvedPair {
            from: history[from_idx].clone(),
            to: history[to_idx].clone(),
        }))
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
        let created_at = created_at.max(0);
        let commit_hash = normalize_commit_hash(commit_hash);
        self.conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;

        let result = (|| -> Result<SirVersionWriteResult, StoreError> {
            let mut latest_stmt = self.conn.prepare(
                r#"
                SELECT version, sir_hash, created_at
                FROM sir_history
                WHERE symbol_id = ?1
                ORDER BY version DESC
                LIMIT 1
                "#,
            )?;

            let latest = latest_stmt
                .query_row(params![symbol_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .optional()?;

            if let Some((latest_version, latest_hash, latest_created_at)) = latest {
                if latest_hash == sir_hash {
                    return Ok(SirVersionWriteResult {
                        version: latest_version,
                        updated_at: latest_created_at,
                        changed: false,
                    });
                }

                let next_version = latest_version + 1;
                self.conn.execute(
                    r#"
                    INSERT INTO sir_history (
                        symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                    )
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    "#,
                    params![
                        symbol_id,
                        next_version,
                        sir_hash,
                        provider,
                        model,
                        created_at,
                        sir_json,
                        commit_hash.as_deref(),
                    ],
                )?;

                return Ok(SirVersionWriteResult {
                    version: next_version,
                    updated_at: created_at,
                    changed: true,
                });
            }

            self.conn.execute(
                r#"
                INSERT INTO sir_history (
                    symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                )
                VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    symbol_id,
                    sir_hash,
                    provider,
                    model,
                    created_at,
                    sir_json,
                    commit_hash.as_deref(),
                ],
            )?;

            Ok(SirVersionWriteResult {
                version: 1,
                updated_at: created_at,
                changed: true,
            })
        })();

        match result {
            Ok(write_result) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(write_result)
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn upsert_symbol_embedding(&self, record: SymbolEmbeddingRecord) -> Result<(), StoreError> {
        let embedding_dim = record.embedding.len() as i64;
        let embedding_json = serde_json::to_string(&record.embedding)?;

        self.conn.execute(
            r#"
            INSERT INTO sir_embeddings (
                symbol_id, sir_hash, provider, model, embedding_dim, embedding_json, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(symbol_id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                provider = excluded.provider,
                model = excluded.model,
                embedding_dim = excluded.embedding_dim,
                embedding_json = excluded.embedding_json,
                updated_at = excluded.updated_at
            "#,
            params![
                record.symbol_id,
                record.sir_hash,
                record.provider,
                record.model,
                embedding_dim,
                embedding_json,
                record.updated_at,
            ],
        )?;

        Ok(())
    }

    fn get_symbol_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolEmbeddingMetaRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, sir_hash, provider, model, embedding_dim, updated_at
            FROM sir_embeddings
            WHERE symbol_id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolEmbeddingMetaRecord {
                    symbol_id: row.get(0)?,
                    sir_hash: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    embedding_dim: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    fn delete_symbol_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM sir_embeddings WHERE symbol_id = ?1",
            params![symbol_id],
        )?;
        Ok(())
    }

    fn search_symbols_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<SemanticSearchResult>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if query_embedding.is_empty() || provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let query_norm_sq = query_embedding
            .iter()
            .map(|value| value * value)
            .fold(0.0f32, |acc, value| acc + value);
        if query_norm_sq <= f32::EPSILON {
            return Ok(Vec::new());
        }
        let query_norm = query_norm_sq.sqrt();
        let capped_limit = limit.clamp(1, 100) as usize;

        let mut stmt = self.conn.prepare(
            r#"
            SELECT symbol_id, embedding_json
            FROM sir_embeddings
            WHERE provider = ?1
              AND model = ?2
              AND embedding_dim = ?3
            "#,
        )?;
        let rows = stmt.query_map(
            params![provider, model, query_embedding.len() as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;

        let mut scored = Vec::new();
        for row in rows {
            let (symbol_id, embedding_json) = row?;
            let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
            if embedding.len() != query_embedding.len() {
                continue;
            }

            let dot = embedding
                .iter()
                .zip(query_embedding.iter())
                .map(|(left, right)| left * right)
                .fold(0.0f32, |acc, value| acc + value);
            let embedding_norm_sq = embedding
                .iter()
                .map(|value| value * value)
                .fold(0.0f32, |acc, value| acc + value);
            if embedding_norm_sq <= f32::EPSILON {
                continue;
            }

            let score = dot / (embedding_norm_sq.sqrt() * query_norm);
            scored.push((symbol_id, score));
        }

        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut results = Vec::new();
        for (symbol_id, score) in scored.into_iter().take(capped_limit) {
            let Some(symbol) = self.get_symbol_search_result(&symbol_id)? else {
                continue;
            };

            results.push(SemanticSearchResult {
                symbol_id: symbol.symbol_id,
                qualified_name: symbol.qualified_name,
                file_path: symbol.file_path,
                language: symbol.language,
                kind: symbol.kind,
                semantic_score: score,
            });
        }

        Ok(results)
    }

    fn upsert_threshold_calibration(
        &self,
        record: ThresholdCalibrationRecord,
    ) -> Result<(), StoreError> {
        let language = record.language.trim().to_ascii_lowercase();
        if language.is_empty() {
            return Ok(());
        }

        self.conn.execute(
            r#"
            INSERT INTO threshold_calibration (
                language, threshold, sample_size, provider, model, calibrated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(language) DO UPDATE SET
                threshold = excluded.threshold,
                sample_size = excluded.sample_size,
                provider = excluded.provider,
                model = excluded.model,
                calibrated_at = excluded.calibrated_at
            "#,
            params![
                language,
                record.threshold,
                record.sample_size,
                record.provider,
                record.model,
                record.calibrated_at
            ],
        )?;

        Ok(())
    }

    fn get_threshold_calibration(
        &self,
        language: &str,
    ) -> Result<Option<ThresholdCalibrationRecord>, StoreError> {
        let language = language.trim().to_ascii_lowercase();
        if language.is_empty() {
            return Ok(None);
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT language, threshold, sample_size, provider, model, calibrated_at
            FROM threshold_calibration
            WHERE language = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![language], |row| {
                Ok(ThresholdCalibrationRecord {
                    language: row.get(0)?,
                    threshold: row.get(1)?,
                    sample_size: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    calibrated_at: row.get(5)?,
                })
            })
            .optional()?;

        Ok(record)
    }

    fn list_threshold_calibrations(&self) -> Result<Vec<ThresholdCalibrationRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT language, threshold, sample_size, provider, model, calibrated_at
            FROM threshold_calibration
            ORDER BY language ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(ThresholdCalibrationRecord {
                language: row.get(0)?,
                threshold: row.get(1)?,
                sample_size: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                calibrated_at: row.get(5)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    fn list_embeddings_for_provider_model(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Vec<CalibrationEmbeddingRecord>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT e.symbol_id, s.file_path, s.language, e.embedding_json
            FROM sir_embeddings e
            JOIN symbols s ON s.id = e.symbol_id
            WHERE e.provider = ?1
              AND e.model = ?2
            ORDER BY s.language ASC, s.file_path ASC, e.symbol_id ASC
            "#,
        )?;

        let rows = stmt.query_map(params![provider, model], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in rows {
            let (symbol_id, file_path, language, embedding_json) = row?;
            let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
            records.push(CalibrationEmbeddingRecord {
                symbol_id,
                file_path,
                language: language.trim().to_ascii_lowercase(),
                embedding,
            });
        }

        Ok(records)
    }

    fn upsert_project_note(&self, record: ProjectNoteRecord) -> Result<(), StoreError> {
        let tags_json = serde_json::to_string(&record.tags)?;
        let entity_refs_json = project_entity_refs_to_json(&record.entity_refs)?;
        let file_refs_json = serde_json::to_string(&record.file_refs)?;
        let symbol_refs_json = serde_json::to_string(&record.symbol_refs)?;
        let archived = if record.is_archived { 1 } else { 0 };

        self.conn.execute(
            r#"
            INSERT INTO project_notes (
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(note_id) DO UPDATE SET
                content = excluded.content,
                content_hash = excluded.content_hash,
                source_type = excluded.source_type,
                source_agent = excluded.source_agent,
                tags = excluded.tags,
                entity_refs = excluded.entity_refs,
                file_refs = excluded.file_refs,
                symbol_refs = excluded.symbol_refs,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                access_count = excluded.access_count,
                last_accessed_at = excluded.last_accessed_at,
                is_archived = excluded.is_archived
            "#,
            params![
                record.note_id,
                record.content,
                record.content_hash,
                record.source_type,
                record.source_agent,
                tags_json,
                entity_refs_json,
                file_refs_json,
                symbol_refs_json,
                record.created_at,
                record.updated_at,
                record.access_count,
                record.last_accessed_at,
                archived,
            ],
        )?;

        Ok(())
    }

    fn find_project_note_by_content_hash(
        &self,
        content_hash: &str,
        include_archived: bool,
    ) -> Result<Option<ProjectNoteRecord>, StoreError> {
        let content_hash = content_hash.trim();
        if content_hash.is_empty() {
            return Ok(None);
        }

        let sql = if include_archived {
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE content_hash = ?1
            ORDER BY updated_at DESC, note_id ASC
            LIMIT 1
            "#
        } else {
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE content_hash = ?1
              AND is_archived = 0
            ORDER BY updated_at DESC, note_id ASC
            LIMIT 1
            "#
        };

        let mut stmt = self.conn.prepare(sql)?;
        let row = stmt
            .query_row(params![content_hash], |row| {
                project_note_tuple_from_row(row)
            })
            .optional()?;

        row.map(project_note_from_tuple).transpose()
    }

    fn get_project_note(&self, note_id: &str) -> Result<Option<ProjectNoteRecord>, StoreError> {
        let note_id = note_id.trim();
        if note_id.is_empty() {
            return Ok(None);
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE note_id = ?1
            LIMIT 1
            "#,
        )?;

        let row = stmt
            .query_row(params![note_id], project_note_tuple_from_row)
            .optional()?;

        row.map(project_note_from_tuple).transpose()
    }

    fn list_project_notes(
        &self,
        limit: u32,
        since_epoch_ms: Option<i64>,
        include_archived: bool,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let mut sql = String::from(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE 1 = 1
            "#,
        );
        let mut params_vec: Vec<SqlValue> = Vec::new();

        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        if let Some(since) = since_epoch_ms {
            sql.push_str(" AND updated_at >= ?");
            params_vec.push(SqlValue::Integer(since.max(0)));
        }

        sql.push_str(" ORDER BY updated_at DESC, note_id ASC LIMIT ?");
        params_vec.push(SqlValue::Integer(limit.clamp(1, 100) as i64));

        let mut stmt = self.conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), project_note_tuple_from_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }

    fn list_project_notes_for_file_ref(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(Vec::new());
        }

        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE is_archived = 0
              AND EXISTS (
                  SELECT 1
                  FROM json_each(project_notes.file_refs)
                  WHERE json_each.value = ?1
              )
            ORDER BY updated_at DESC, note_id ASC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(
            params![file_path, limit.clamp(1, 100)],
            project_note_tuple_from_row,
        )?;
        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }

    fn search_project_notes_lexical(
        &self,
        query: &str,
        limit: u32,
        include_archived: bool,
        tags_filter: &[String],
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let query = query.trim();
        let mut sql = String::from(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE 1 = 1
            "#,
        );
        let mut params_vec: Vec<SqlValue> = Vec::new();

        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        if !query.is_empty() {
            let terms = project_note_lexical_terms(query);
            if !terms.is_empty() {
                sql.push_str(" AND (");
                for (index, term) in terms.iter().enumerate() {
                    if index > 0 {
                        sql.push_str(" OR ");
                    }
                    sql.push_str("(LOWER(content) LIKE ? OR LOWER(tags) LIKE ?)");

                    let pattern = format!("%{term}%");
                    params_vec.push(SqlValue::Text(pattern.clone()));
                    params_vec.push(SqlValue::Text(pattern));
                }
                sql.push(')');
            }
        }

        for tag in tags_filter
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
        {
            sql.push_str(" AND LOWER(tags) LIKE LOWER(?)");
            params_vec.push(SqlValue::Text(format!("%\"{tag}\"%")));
        }

        sql.push_str(" ORDER BY updated_at DESC, note_id ASC LIMIT ?");
        params_vec.push(SqlValue::Integer(limit.clamp(1, 100) as i64));

        let mut stmt = self.conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), project_note_tuple_from_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }

    fn increment_project_note_access(
        &self,
        note_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        if note_ids.is_empty() {
            return Ok(());
        }

        self.conn.execute_batch("BEGIN IMMEDIATE TRANSACTION")?;
        let result = (|| -> Result<(), StoreError> {
            let mut stmt = self.conn.prepare(
                r#"
                UPDATE project_notes
                SET access_count = access_count + 1,
                    last_accessed_at = ?2
                WHERE note_id = ?1
                "#,
            )?;

            for note_id in note_ids {
                let trimmed = note_id.trim();
                if trimmed.is_empty() {
                    continue;
                }
                stmt.execute(params![trimmed, accessed_at.max(0)])?;
            }

            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn upsert_project_note_embedding(
        &self,
        record: ProjectNoteEmbeddingRecord,
    ) -> Result<(), StoreError> {
        if record.embedding.is_empty() {
            return Ok(());
        }

        self.conn.execute(
            r#"
            INSERT INTO project_notes_embeddings (
                note_id, provider, model, embedding_dim, embedding_json, content, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(note_id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                embedding_dim = excluded.embedding_dim,
                embedding_json = excluded.embedding_json,
                content = excluded.content,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            "#,
            params![
                record.note_id,
                record.provider,
                record.model,
                record.embedding.len() as i64,
                serde_json::to_string(&record.embedding)?,
                record.content,
                record.created_at.max(0),
                record.updated_at.max(0),
            ],
        )?;

        Ok(())
    }

    fn delete_project_note_embedding(&self, note_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM project_notes_embeddings WHERE note_id = ?1",
            params![note_id],
        )?;
        Ok(())
    }

    fn search_project_notes_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteSemanticSearchResult>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if query_embedding.is_empty() || provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let query_norm_sq = query_embedding
            .iter()
            .map(|value| value * value)
            .fold(0.0f32, |acc, value| acc + value);
        if query_norm_sq <= f32::EPSILON {
            return Ok(Vec::new());
        }
        let query_norm = query_norm_sq.sqrt();
        let capped_limit = limit.clamp(1, 100) as usize;

        let mut stmt = self.conn.prepare(
            r#"
            SELECT note_id, embedding_json
            FROM project_notes_embeddings
            WHERE provider = ?1
              AND model = ?2
              AND embedding_dim = ?3
            "#,
        )?;
        let rows = stmt.query_map(
            params![provider, model, query_embedding.len() as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;

        let mut scored = Vec::new();
        for row in rows {
            let (note_id, embedding_json) = row?;
            let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
            if embedding.len() != query_embedding.len() {
                continue;
            }

            let dot = embedding
                .iter()
                .zip(query_embedding.iter())
                .map(|(left, right)| left * right)
                .fold(0.0f32, |acc, value| acc + value);
            let embedding_norm_sq = embedding
                .iter()
                .map(|value| value * value)
                .fold(0.0f32, |acc, value| acc + value);
            if embedding_norm_sq <= f32::EPSILON {
                continue;
            }

            let score = dot / (embedding_norm_sq.sqrt() * query_norm);
            scored.push(ProjectNoteSemanticSearchResult {
                note_id,
                semantic_score: score,
            });
        }

        scored.sort_by(|left, right| {
            right
                .semantic_score
                .partial_cmp(&left.semantic_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        scored.truncate(capped_limit);
        Ok(scored)
    }

    fn get_coupling_mining_state(&self) -> Result<Option<CouplingMiningStateRecord>, StoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT last_commit_hash, last_mined_at, commits_scanned
            FROM coupling_mining_state
            WHERE id = 1
            LIMIT 1
            "#,
        )?;

        stmt.query_row([], |row| {
            Ok(CouplingMiningStateRecord {
                last_commit_hash: row.get(0)?,
                last_mined_at: row.get(1)?,
                commits_scanned: row.get::<_, i64>(2)?.max(0),
            })
        })
        .optional()
        .map_err(Into::into)
    }

    fn upsert_coupling_mining_state(
        &self,
        state: CouplingMiningStateRecord,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            r#"
            INSERT INTO coupling_mining_state (id, last_commit_hash, last_mined_at, commits_scanned)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                last_commit_hash = excluded.last_commit_hash,
                last_mined_at = excluded.last_mined_at,
                commits_scanned = excluded.commits_scanned
            "#,
            params![
                state.last_commit_hash,
                state.last_mined_at.map(|value| value.max(0)),
                state.commits_scanned.max(0),
            ],
        )?;

        Ok(())
    }

    fn has_dependency_between_files(&self, file_a: &str, file_b: &str) -> Result<bool, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(false);
        }

        let exists = self.conn.query_row(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM symbol_edges e
                JOIN symbols s_source ON s_source.id = e.source_id
                JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name
                WHERE e.edge_kind IN ('calls', 'depends_on')
                  AND (
                      (s_source.file_path = ?1 AND s_target.file_path = ?2)
                      OR
                      (s_source.file_path = ?2 AND s_target.file_path = ?1)
                  )
                LIMIT 1
            )
            "#,
            params![file_a, file_b],
            |row| row.get::<_, i64>(0),
        )?;

        Ok(exists != 0)
    }
}

type ProjectNoteRowTuple = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    i64,
);

fn project_note_tuple_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectNoteRowTuple> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
    ))
}

fn project_note_lexical_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();

    for token in query
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '/'))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let token = token.to_ascii_lowercase();
        if !terms.iter().any(|existing| existing == &token) {
            terms.push(token);
        }
    }

    if terms.is_empty() && !query.trim().is_empty() {
        terms.push(query.trim().to_ascii_lowercase());
    }

    terms
}

fn project_note_from_tuple(tuple: ProjectNoteRowTuple) -> Result<ProjectNoteRecord, StoreError> {
    let (
        note_id,
        content,
        content_hash,
        source_type,
        source_agent,
        tags_json,
        entity_refs_json,
        file_refs_json,
        symbol_refs_json,
        created_at,
        updated_at,
        access_count,
        last_accessed_at,
        is_archived,
    ) = tuple;

    Ok(ProjectNoteRecord {
        note_id,
        content,
        content_hash,
        source_type,
        source_agent,
        tags: parse_string_array_json(&tags_json)?,
        entity_refs: parse_project_entity_refs_json(&entity_refs_json)?,
        file_refs: parse_string_array_json(&file_refs_json)?,
        symbol_refs: parse_string_array_json(&symbol_refs_json)?,
        created_at,
        updated_at,
        access_count,
        last_accessed_at,
        is_archived: is_archived != 0,
    })
}

fn parse_string_array_json(raw: &str) -> Result<Vec<String>, StoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    Ok(json_from_str::<Vec<String>>(trimmed)?)
}

fn parse_project_entity_refs_json(raw: &str) -> Result<Vec<ProjectEntityRefRecord>, StoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let value = serde_json::from_str::<serde_json::Value>(trimmed)?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };

    let mut refs = Vec::new();
    for item in items {
        let kind = item.get("kind").and_then(serde_json::Value::as_str);
        let id = item.get("id").and_then(serde_json::Value::as_str);
        let (Some(kind), Some(id)) = (kind, id) else {
            continue;
        };

        let kind = kind.trim();
        let id = id.trim();
        if kind.is_empty() || id.is_empty() {
            continue;
        }

        refs.push(ProjectEntityRefRecord {
            kind: kind.to_owned(),
            id: id.to_owned(),
        });
    }

    Ok(refs)
}

fn project_entity_refs_to_json(
    entity_refs: &[ProjectEntityRefRecord],
) -> Result<String, StoreError> {
    let values = entity_refs
        .iter()
        .map(|entity| {
            serde_json::json!({
                "kind": entity.kind,
                "id": entity.id,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string(&values)?)
}

fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS symbols (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            language TEXT NOT NULL,
            kind TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            signature_fingerprint TEXT NOT NULL,
            last_seen_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sir (
            id TEXT PRIMARY KEY,
            sir_hash TEXT NOT NULL,
            sir_version INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            sir_json TEXT
        );

        CREATE TABLE IF NOT EXISTS sir_history (
            symbol_id TEXT NOT NULL,
            version INTEGER NOT NULL CHECK (version >= 1),
            sir_hash TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            sir_json TEXT NOT NULL,
            commit_hash TEXT CHECK (
                commit_hash IS NULL
                OR (
                    LENGTH(commit_hash) = 40
                    AND commit_hash NOT GLOB '*[^0-9a-f]*'
                )
            ),
            PRIMARY KEY (symbol_id, version)
        );

        CREATE INDEX IF NOT EXISTS idx_sir_history_symbol_created_version
            ON sir_history(symbol_id, created_at ASC, version ASC);

        CREATE INDEX IF NOT EXISTS idx_sir_history_symbol_latest
            ON sir_history(symbol_id, version DESC);

        CREATE TABLE IF NOT EXISTS sir_embeddings (
            symbol_id TEXT PRIMARY KEY,
            sir_hash TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            embedding_dim INTEGER NOT NULL,
            embedding_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_sir_embeddings_provider_model_dim
            ON sir_embeddings(provider, model, embedding_dim);

        CREATE TABLE IF NOT EXISTS threshold_calibration (
            language TEXT PRIMARY KEY,
            threshold REAL NOT NULL,
            sample_size INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            calibrated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_threshold_calibration_provider_model
            ON threshold_calibration(provider, model);

        CREATE TABLE IF NOT EXISTS project_notes (
            note_id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            source_type TEXT NOT NULL,
            source_agent TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            entity_refs TEXT NOT NULL DEFAULT '[]',
            file_refs TEXT NOT NULL DEFAULT '[]',
            symbol_refs TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            last_accessed_at INTEGER,
            is_archived INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_project_notes_content_hash
            ON project_notes(content_hash);
        CREATE INDEX IF NOT EXISTS idx_project_notes_source_type
            ON project_notes(source_type);
        CREATE INDEX IF NOT EXISTS idx_project_notes_created_at
            ON project_notes(created_at);
        CREATE INDEX IF NOT EXISTS idx_project_notes_archived
            ON project_notes(is_archived);

        CREATE TABLE IF NOT EXISTS project_notes_embeddings (
            note_id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            embedding_dim INTEGER NOT NULL,
            embedding_json TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_project_notes_embeddings_provider_model_dim
            ON project_notes_embeddings(provider, model, embedding_dim);

        CREATE TABLE IF NOT EXISTS symbol_edges (
            source_id TEXT NOT NULL,
            target_qualified_name TEXT NOT NULL,
            edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on')),
            file_path TEXT NOT NULL,
            PRIMARY KEY (source_id, target_qualified_name, edge_kind)
        );

        CREATE INDEX IF NOT EXISTS idx_edges_target
            ON symbol_edges(target_qualified_name);

        CREATE INDEX IF NOT EXISTS idx_edges_file
            ON symbol_edges(file_path);

        CREATE TABLE IF NOT EXISTS coupling_mining_state (
            id INTEGER PRIMARY KEY DEFAULT 1,
            last_commit_hash TEXT,
            last_mined_at INTEGER,
            commits_scanned INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )?;

    if !table_has_column(conn, "sir", "sir_json")? {
        conn.execute("ALTER TABLE sir ADD COLUMN sir_json TEXT", [])?;
    }

    ensure_sir_column(conn, "sir_status", "TEXT NOT NULL DEFAULT 'fresh'")?;
    ensure_sir_column(conn, "last_error", "TEXT")?;
    ensure_sir_column(conn, "last_attempt_at", "INTEGER NOT NULL DEFAULT 0")?;
    ensure_sir_history_column(conn, "commit_hash", "TEXT")?;

    conn.execute(
        "UPDATE sir SET sir_status = 'fresh' WHERE COALESCE(TRIM(sir_status), '') = ''",
        [],
    )?;
    conn.execute(
        "UPDATE sir SET last_attempt_at = updated_at WHERE last_attempt_at = 0",
        [],
    )?;
    conn.execute(
        r#"
        INSERT INTO sir_history (
            symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
        )
        SELECT
            s.id,
            CASE WHEN s.sir_version > 0 THEN s.sir_version ELSE 1 END,
            s.sir_hash,
            s.provider,
            s.model,
            CASE WHEN s.updated_at > 0 THEN s.updated_at ELSE unixepoch() END,
            s.sir_json,
            NULL
        FROM sir s
        WHERE COALESCE(TRIM(s.sir_hash), '') <> ''
          AND COALESCE(TRIM(s.sir_json), '') <> ''
          AND NOT EXISTS (
              SELECT 1 FROM sir_history h WHERE h.symbol_id = s.id
          )
        "#,
        [],
    )?;

    Ok(())
}

fn ensure_sir_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if table_has_column(conn, "sir", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE sir ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}

fn ensure_sir_history_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if table_has_column(conn, "sir_history", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE sir_history ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}

fn table_has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, StoreError> {
    let sql = format!("PRAGMA table_info({table_name})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for row in rows {
        if row?.eq_ignore_ascii_case(column_name) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn normalize_commit_hash(commit_hash: Option<&str>) -> Option<String> {
    let value = commit_hash?.trim();
    if value.len() != 40 {
        return None;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }

    Some(value.to_owned())
}

fn resolve_history_selector_index(
    history: &[SirHistoryRecord],
    selector: &SirHistorySelector,
) -> Option<usize> {
    match selector {
        SirHistorySelector::Version(version) => {
            history.iter().position(|record| record.version == *version)
        }
        SirHistorySelector::CreatedAt(created_at) => history
            .iter()
            .enumerate()
            .filter(|(_, record)| record.created_at <= *created_at)
            .map(|(idx, _)| idx)
            .next_back(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn symbol_record() -> SymbolRecord {
        SymbolRecord {
            id: "sym-1".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "demo::run".to_owned(),
            signature_fingerprint: "sig-a".to_owned(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn sir_meta_record() -> SirMetaRecord {
        SirMetaRecord {
            id: "sym-1".to_owned(),
            sir_hash: "hash-a".to_owned(),
            sir_version: 1,
            provider: "none".to_owned(),
            model: "none".to_owned(),
            updated_at: 1_700_000_100,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_000_100,
        }
    }

    fn symbol_record_ts() -> SymbolRecord {
        SymbolRecord {
            id: "sym-2".to_owned(),
            file_path: "src/app.ts".to_owned(),
            language: "typescript".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "web::render".to_owned(),
            signature_fingerprint: "sig-c".to_owned(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn embedding_record(
        symbol_id: &str,
        sir_hash: &str,
        embedding: Vec<f32>,
    ) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: sir_hash.to_owned(),
            provider: "mock".to_owned(),
            model: "mock-64d".to_owned(),
            embedding,
            updated_at: 1_700_000_500,
        }
    }

    fn project_note_record(
        note_id: &str,
        content: &str,
        tags: &[&str],
        updated_at: i64,
    ) -> ProjectNoteRecord {
        ProjectNoteRecord {
            note_id: note_id.to_owned(),
            content: content.to_owned(),
            content_hash: format!("hash-{note_id}"),
            source_type: "manual".to_owned(),
            source_agent: None,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            entity_refs: Vec::new(),
            file_refs: Vec::new(),
            symbol_refs: Vec::new(),
            created_at: updated_at,
            updated_at,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        }
    }

    fn calls_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
        SymbolEdge {
            source_id: source_id.to_owned(),
            target_qualified_name: target.to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: file_path.to_owned(),
        }
    }

    fn depends_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
        SymbolEdge {
            source_id: source_id.to_owned(),
            target_qualified_name: target.to_owned(),
            edge_kind: EdgeKind::DependsOn,
            file_path: file_path.to_owned(),
        }
    }

    #[test]
    fn store_creates_layout_and_persists_data_without_duplicates() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let store = SqliteStore::open(workspace).expect("open store");
        assert!(store.aether_dir().exists());
        assert!(store.sir_dir().exists());
        assert!(store.mirror_sir_files_enabled());
        assert!(store.aether_dir().join("meta.sqlite").exists());

        let mut record = symbol_record();
        store
            .upsert_symbol(record.clone())
            .expect("upsert symbol first time");

        record.last_seen_at = 1_700_000_200;
        record.signature_fingerprint = "sig-b".to_owned();
        store
            .upsert_symbol(record.clone())
            .expect("upsert symbol second time");

        let list = store
            .list_symbols_for_file("src/lib.rs")
            .expect("list symbols after upsert");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], record);

        store
            .write_sir_blob("sym-1", "{\"intent\":\"demo\"}")
            .expect("write blob");
        let blob = store.read_sir_blob("sym-1").expect("read blob");
        assert_eq!(blob.as_deref(), Some("{\"intent\":\"demo\"}"));

        let sir_meta = sir_meta_record();
        store
            .upsert_sir_meta(sir_meta.clone())
            .expect("upsert sir meta");
        let loaded_meta = store.get_sir_meta("sym-1").expect("get sir meta");
        assert_eq!(loaded_meta, Some(sir_meta));

        drop(store);

        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let reopened_list = reopened
            .list_symbols_for_file("src/lib.rs")
            .expect("list symbols after reopen");
        assert_eq!(reopened_list.len(), 1);
        assert_eq!(reopened_list[0], record);

        let reopened_blob = reopened
            .read_sir_blob("sym-1")
            .expect("read blob after reopen");
        assert_eq!(reopened_blob.as_deref(), Some("{\"intent\":\"demo\"}"));

        let reopened_meta = reopened.get_sir_meta("sym-1").expect("meta after reopen");
        assert_eq!(reopened_meta, Some(sir_meta_record()));
    }

    #[test]
    fn embedding_records_persist_and_search_semantic_ranks_expected_match() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert first symbol");
        let mut second = symbol_record_ts();
        second.id = "sym-2".to_owned();
        second.qualified_name = "demo::network_retry".to_owned();
        store.upsert_symbol(second).expect("upsert second symbol");

        store
            .upsert_symbol_embedding(embedding_record("sym-1", "hash-a", vec![1.0, 0.0]))
            .expect("upsert first embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-2", "hash-b", vec![0.0, 1.0]))
            .expect("upsert second embedding");

        let meta = store
            .get_symbol_embedding_meta("sym-1")
            .expect("read embedding meta")
            .expect("embedding meta exists");
        assert_eq!(meta.sir_hash, "hash-a");
        assert_eq!(meta.embedding_dim, 2);

        let semantic = store
            .search_symbols_semantic(&[0.0, 1.0], "mock", "mock-64d", 5)
            .expect("semantic search");
        assert!(!semantic.is_empty());
        assert_eq!(semantic[0].symbol_id, "sym-2");
        assert!(semantic[0].semantic_score > semantic[1].semantic_score);
    }

    #[test]
    fn edge_records_can_be_upserted_queried_and_deleted_by_file() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_edges(&[
                calls_edge("sym-alpha", "beta", "src/lib.rs"),
                depends_edge("file::src/app.ts", "./dep", "src/app.ts"),
            ])
            .expect("upsert edges");

        let callers = store.get_callers("beta").expect("get callers");
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0], calls_edge("sym-alpha", "beta", "src/lib.rs"));

        let deps = store
            .get_dependencies("file::src/app.ts")
            .expect("get dependencies");
        assert_eq!(deps.len(), 1);
        assert_eq!(
            deps[0],
            depends_edge("file::src/app.ts", "./dep", "src/app.ts")
        );

        store
            .delete_edges_for_file("src/lib.rs")
            .expect("delete edges for file");
        let callers_after_delete = store.get_callers("beta").expect("get callers after delete");
        assert!(callers_after_delete.is_empty());

        let deps_after_delete = store
            .get_dependencies("file::src/app.ts")
            .expect("get dependencies after delete");
        assert_eq!(deps_after_delete.len(), 1);
    }

    #[test]
    fn sync_graph_for_file_skips_unresolved_calls() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = SymbolRecord {
            id: "sym-alpha".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "alpha".to_owned(),
            signature_fingerprint: "sig-alpha".to_owned(),
            last_seen_at: 1_700_000_000,
        };
        store.upsert_symbol(alpha.clone()).expect("upsert symbol");
        store
            .upsert_edges(&[calls_edge(&alpha.id, "missing::target", "src/lib.rs")])
            .expect("upsert unresolved edge");

        let stats = store
            .sync_graph_for_file(&graph, "src/lib.rs")
            .expect("sync graph for file");
        assert_eq!(stats.resolved_edges, 0);
        assert_eq!(stats.unresolved_edges, 1);

        let deps = graph
            .get_dependencies(&alpha.id)
            .expect("query dependencies");
        assert!(deps.is_empty());
    }

    #[test]
    fn graph_backend_config_toggle_switches_between_sqlite_and_cozo() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
mirror_sir_files = true
graph_backend = "sqlite"
"#,
        )
        .expect("write sqlite graph config");

        let store = SqliteStore::open(workspace).expect("open store");
        let alpha = SymbolRecord {
            id: "sym-alpha".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "alpha".to_owned(),
            signature_fingerprint: "sig-alpha".to_owned(),
            last_seen_at: 1_700_000_000,
        };
        let beta = SymbolRecord {
            id: "sym-beta".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "beta".to_owned(),
            signature_fingerprint: "sig-beta".to_owned(),
            last_seen_at: 1_700_000_000,
        };
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store.upsert_symbol(beta.clone()).expect("upsert beta");
        store
            .upsert_edges(&[calls_edge(&alpha.id, "beta", "src/lib.rs")])
            .expect("upsert call edge");

        let sqlite_graph = open_graph_store(workspace).expect("open sqlite graph backend");
        let sqlite_deps = sqlite_graph
            .get_dependencies(&alpha.id)
            .expect("query sqlite backend");
        assert_eq!(sqlite_deps.len(), 1);
        assert_eq!(sqlite_deps[0].id, beta.id);

        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
mirror_sir_files = true
graph_backend = "cozo"
"#,
        )
        .expect("write cozo graph config");

        let cozo_graph = open_graph_store(workspace).expect("open cozo graph backend");
        let stats = store
            .sync_graph_for_file(cozo_graph.as_ref(), "src/lib.rs")
            .expect("sync cozo graph");
        assert_eq!(stats.resolved_edges, 1);
        assert_eq!(stats.unresolved_edges, 0);

        let cozo_deps = cozo_graph
            .get_dependencies(&alpha.id)
            .expect("query cozo backend");
        assert_eq!(cozo_deps.len(), 1);
        assert_eq!(cozo_deps[0].id, beta.id);
    }

    #[test]
    fn read_sir_blob_prefers_sqlite_when_mirror_is_missing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        store
            .write_sir_blob("sym-1", "{\"intent\":\"db-primary\"}")
            .expect("write blob");

        let mirror_path = workspace.join(".aether/sir/sym-1.json");
        fs::remove_file(&mirror_path).expect("remove mirror");

        let loaded = store.read_sir_blob("sym-1").expect("read from sqlite");
        assert_eq!(loaded.as_deref(), Some("{\"intent\":\"db-primary\"}"));

        drop(store);

        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let reopened_loaded = reopened.read_sir_blob("sym-1").expect("read after reopen");
        assert_eq!(
            reopened_loaded.as_deref(),
            Some("{\"intent\":\"db-primary\"}")
        );
    }

    #[test]
    fn read_sir_blob_backfills_sqlite_from_mirror_without_overwriting_meta() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let meta = SirMetaRecord {
            id: "sym-legacy".to_owned(),
            sir_hash: "legacy-hash".to_owned(),
            sir_version: 3,
            provider: "legacy-provider".to_owned(),
            model: "legacy-model".to_owned(),
            updated_at: 1_700_111_222,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_111_222,
        };
        store
            .upsert_sir_meta(meta.clone())
            .expect("upsert legacy metadata");

        let mirror_path = workspace.join(".aether/sir/sym-legacy.json");
        fs::write(&mirror_path, "{\"intent\":\"from-mirror\"}").expect("write mirror");

        let first_read = store.read_sir_blob("sym-legacy").expect("first read");
        assert_eq!(first_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

        fs::remove_file(&mirror_path).expect("remove mirror");

        let second_read = store.read_sir_blob("sym-legacy").expect("second read");
        assert_eq!(second_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

        let meta_after = store
            .get_sir_meta("sym-legacy")
            .expect("read metadata after backfill");
        assert_eq!(meta_after, Some(meta));
    }

    #[test]
    fn sir_history_records_are_ordered_and_persist_after_reopen() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let first = store
            .record_sir_version_if_changed(
                "sym-history",
                "hash-1",
                "mock",
                "mock",
                "{\"intent\":\"v1\"}",
                1_700_222_100,
                Some("1111111111111111111111111111111111111111"),
            )
            .expect("insert history v1");
        assert!(first.changed);
        assert_eq!(first.version, 1);

        let duplicate = store
            .record_sir_version_if_changed(
                "sym-history",
                "hash-1",
                "mock",
                "mock",
                "{\"intent\":\"v1\"}",
                1_700_222_101,
                Some("1111111111111111111111111111111111111111"),
            )
            .expect("dedupe by hash");
        assert!(!duplicate.changed);
        assert_eq!(duplicate.version, 1);
        assert_eq!(duplicate.updated_at, first.updated_at);

        let second = store
            .record_sir_version_if_changed(
                "sym-history",
                "hash-2",
                "mock",
                "mock",
                "{\"intent\":\"v2\"}",
                1_700_222_200,
                Some("2222222222222222222222222222222222222222"),
            )
            .expect("insert history v2");
        assert!(second.changed);
        assert_eq!(second.version, 2);

        let history = store
            .list_sir_history("sym-history")
            .expect("list ordered history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].version, 1);
        assert_eq!(history[0].sir_hash, "hash-1");
        assert_eq!(
            history[0].commit_hash.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!(history[1].version, 2);
        assert_eq!(history[1].sir_hash, "hash-2");
        assert_eq!(
            history[1].commit_hash.as_deref(),
            Some("2222222222222222222222222222222222222222")
        );

        drop(store);

        let reopened = SqliteStore::open(workspace).expect("reopen store");
        let reopened_history = reopened
            .list_sir_history("sym-history")
            .expect("list history after reopen");
        assert_eq!(reopened_history, history);
    }

    #[test]
    fn resolve_sir_history_pair_supports_versions_and_timestamps() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        store
            .record_sir_version_if_changed(
                "sym-history",
                "hash-1",
                "mock",
                "mock",
                "{\"intent\":\"v1\"}",
                1_700_300_100,
                Some("1111111111111111111111111111111111111111"),
            )
            .expect("insert history v1");
        store
            .record_sir_version_if_changed(
                "sym-history",
                "hash-2",
                "mock",
                "mock",
                "{\"intent\":\"v2\"}",
                1_700_300_200,
                Some("2222222222222222222222222222222222222222"),
            )
            .expect("insert history v2");

        let by_version = store
            .resolve_sir_history_pair(
                "sym-history",
                SirHistorySelector::Version(1),
                SirHistorySelector::Version(2),
            )
            .expect("resolve by version")
            .expect("pair should exist");
        assert_eq!(by_version.from.version, 1);
        assert_eq!(by_version.to.version, 2);
        assert_eq!(
            by_version.from.commit_hash.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!(
            by_version.to.commit_hash.as_deref(),
            Some("2222222222222222222222222222222222222222")
        );

        let by_timestamp = store
            .resolve_sir_history_pair(
                "sym-history",
                SirHistorySelector::CreatedAt(1_700_300_150),
                SirHistorySelector::CreatedAt(1_700_300_250),
            )
            .expect("resolve by timestamp")
            .expect("timestamp pair should exist");
        assert_eq!(by_timestamp.from.version, 1);
        assert_eq!(by_timestamp.to.version, 2);

        let unresolved = store
            .resolve_sir_history_pair(
                "sym-history",
                SirHistorySelector::Version(2),
                SirHistorySelector::Version(1),
            )
            .expect("resolve reversed pair");
        assert!(unresolved.is_none());
    }

    #[test]
    fn latest_sir_history_pair_handles_empty_single_and_multiple_history() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let empty = store
            .latest_sir_history_pair("missing")
            .expect("query empty history");
        assert!(empty.is_none());

        store
            .record_sir_version_if_changed(
                "sym-latest",
                "hash-1",
                "mock",
                "mock",
                "{\"intent\":\"v1\"}",
                1_700_310_100,
                None,
            )
            .expect("insert single version");
        let single = store
            .latest_sir_history_pair("sym-latest")
            .expect("query single history")
            .expect("single pair");
        assert_eq!(single.from.version, 1);
        assert_eq!(single.to.version, 1);

        store
            .record_sir_version_if_changed(
                "sym-latest",
                "hash-2",
                "mock",
                "mock",
                "{\"intent\":\"v2\"}",
                1_700_310_200,
                None,
            )
            .expect("insert second version");
        let multiple = store
            .latest_sir_history_pair("sym-latest")
            .expect("query multiple history")
            .expect("multiple pair");
        assert_eq!(multiple.from.version, 1);
        assert_eq!(multiple.to.version, 2);
    }

    #[test]
    fn mirror_write_can_be_disabled_via_config() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = false
"#,
        )
        .expect("write config");

        let store = SqliteStore::open(workspace).expect("open store");
        assert!(!store.mirror_sir_files_enabled());

        store
            .write_sir_blob("sym-1", "{\"intent\":\"sqlite-only\"}")
            .expect("write sqlite-only");

        let mirror_path = workspace.join(".aether/sir/sym-1.json");
        assert!(!mirror_path.exists());

        let loaded = store.read_sir_blob("sym-1").expect("read sqlite-only");
        assert_eq!(loaded.as_deref(), Some("{\"intent\":\"sqlite-only\"}"));
    }

    #[test]
    fn mark_removed_deletes_symbol_row() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert symbol before delete");
        store
            .write_sir_blob("sym-1", "{\"intent\":\"to-remove\"}")
            .expect("write sir before delete");
        store
            .upsert_symbol_embedding(embedding_record("sym-1", "hash-remove", vec![1.0, 0.0]))
            .expect("write embedding before delete");
        store
            .record_sir_version_if_changed(
                "sym-1",
                "hash-remove",
                "mock",
                "mock",
                "{\"intent\":\"to-remove\"}",
                1_700_111_000,
                None,
            )
            .expect("insert history before delete");
        store.mark_removed("sym-1").expect("mark removed");

        let list = store
            .list_symbols_for_file("src/lib.rs")
            .expect("list after delete");
        assert!(list.is_empty());

        let sir = store.read_sir_blob("sym-1").expect("sir after delete");
        assert!(sir.is_none());

        let embedding_meta = store
            .get_symbol_embedding_meta("sym-1")
            .expect("embedding metadata after delete");
        assert!(embedding_meta.is_none());

        let history = store
            .list_sir_history("sym-1")
            .expect("history after delete");
        assert!(history.is_empty());
    }

    #[test]
    fn search_symbols_matches_by_name_path_language_and_kind() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(symbol_record())
            .expect("upsert rust symbol");
        store
            .upsert_symbol(symbol_record_ts())
            .expect("upsert ts symbol");

        let by_name = store
            .search_symbols("demo::run", 20)
            .expect("search by name");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].symbol_id, "sym-1");

        let by_path = store
            .search_symbols("src/app.ts", 20)
            .expect("search by path");
        assert_eq!(by_path.len(), 1);
        assert_eq!(by_path[0].symbol_id, "sym-2");

        let by_language = store
            .search_symbols("RUST", 20)
            .expect("search by language");
        assert_eq!(by_language.len(), 1);
        assert_eq!(by_language[0].symbol_id, "sym-1");

        let by_kind = store
            .search_symbols("function", 20)
            .expect("search by kind");
        assert_eq!(by_kind.len(), 2);
    }

    #[test]
    fn search_symbols_respects_empty_query_and_limit() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        let mut first = symbol_record();
        first.qualified_name = "alpha::run".to_owned();
        first.id = "sym-a".to_owned();
        store.upsert_symbol(first).expect("upsert first symbol");

        let mut second = symbol_record();
        second.qualified_name = "beta::run".to_owned();
        second.id = "sym-b".to_owned();
        store.upsert_symbol(second).expect("upsert second symbol");

        let empty = store.search_symbols("   ", 20).expect("search empty");
        assert!(empty.is_empty());

        let limited = store.search_symbols("::run", 1).expect("search with limit");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].qualified_name, "alpha::run");
    }

    #[test]
    fn search_project_notes_lexical_matches_query_terms() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_project_note(project_note_record(
                "note-1",
                "We selected sqlite for deterministic local persistence.",
                &["architecture"],
                1_700_000_000,
            ))
            .expect("upsert project note");

        let matches = store
            .search_project_notes_lexical("why sqlite", 10, false, &[])
            .expect("search project notes lexically");

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].note_id, "note-1");
    }

    #[test]
    fn open_store_backfills_sir_history_from_existing_sir_rows() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let aether_dir = workspace.join(".aether");
        let sir_dir = aether_dir.join("sir");
        fs::create_dir_all(&sir_dir).expect("create legacy aether dirs");

        let sqlite_path = aether_dir.join("meta.sqlite");
        let conn = Connection::open(&sqlite_path).expect("open legacy sqlite");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                signature_fingerprint TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sir (
                id TEXT PRIMARY KEY,
                sir_hash TEXT NOT NULL,
                sir_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                sir_json TEXT
            );
            "#,
        )
        .expect("create legacy schema with sir_json");

        conn.execute(
            "INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "legacy-history",
                "legacy-hash",
                3i64,
                "mock",
                "mock",
                1_700_222_333i64,
                "{\"intent\":\"legacy\"}"
            ],
        )
        .expect("insert legacy sir row with json");
        drop(conn);

        let store = SqliteStore::open(workspace).expect("open migrated store");
        let history = store
            .list_sir_history("legacy-history")
            .expect("load migrated history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].version, 3);
        assert_eq!(history[0].sir_hash, "legacy-hash");
        assert_eq!(history[0].sir_json, "{\"intent\":\"legacy\"}");
        assert_eq!(history[0].created_at, 1_700_222_333);
        assert_eq!(history[0].commit_hash, None);
    }

    #[test]
    fn open_store_migrates_legacy_sir_table_with_stale_defaults() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let aether_dir = workspace.join(".aether");
        let sir_dir = aether_dir.join("sir");
        fs::create_dir_all(&sir_dir).expect("create legacy aether dirs");

        let sqlite_path = aether_dir.join("meta.sqlite");
        let conn = Connection::open(&sqlite_path).expect("open legacy sqlite");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                signature_fingerprint TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sir (
                id TEXT PRIMARY KEY,
                sir_hash TEXT NOT NULL,
                sir_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .expect("create legacy schema");

        conn.execute(
            "INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["legacy-sym", "legacy-hash", 1i64, "mock", "mock", 1_700_000_500i64],
        )
        .expect("insert legacy sir row");
        drop(conn);

        let store = SqliteStore::open(workspace).expect("open migrated store");
        let migrated = store
            .get_sir_meta("legacy-sym")
            .expect("load migrated row")
            .expect("row exists");

        assert_eq!(migrated.sir_status, "fresh");
        assert_eq!(migrated.last_error, None);
        assert_eq!(migrated.last_attempt_at, migrated.updated_at);
        assert!(
            store
                .list_sir_history("legacy-sym")
                .expect("load history for legacy row without sir_json")
                .is_empty()
        );

        let embedding_lookup = store
            .search_symbols_semantic(&[1.0, 0.0], "mock", "mock-64d", 10)
            .expect("semantic search on migrated schema");
        assert!(embedding_lookup.is_empty());
    }

    #[test]
    fn threshold_calibration_round_trip_persists_latest_value() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_threshold_calibration(ThresholdCalibrationRecord {
                language: "rust".to_owned(),
                threshold: 0.72,
                sample_size: 123,
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                calibrated_at: "2026-02-19T00:00:00Z".to_owned(),
            })
            .expect("upsert threshold");
        store
            .upsert_threshold_calibration(ThresholdCalibrationRecord {
                language: "rust".to_owned(),
                threshold: 0.74,
                sample_size: 456,
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                calibrated_at: "2026-02-19T00:01:00Z".to_owned(),
            })
            .expect("upsert threshold update");

        let rust = store
            .get_threshold_calibration("rust")
            .expect("get threshold")
            .expect("threshold exists");
        assert_eq!(rust.threshold, 0.74);
        assert_eq!(rust.sample_size, 456);
        assert_eq!(rust.provider, "mock");

        let all = store
            .list_threshold_calibrations()
            .expect("list threshold calibrations");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].language, "rust");
    }

    #[test]
    fn list_embeddings_for_provider_model_returns_language_context() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_symbol(SymbolRecord {
                id: "sym-rust".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::run".to_owned(),
                signature_fingerprint: "sig-rust".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert rust symbol");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-py".to_owned(),
                file_path: "src/jobs.py".to_owned(),
                language: "python".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "jobs.run".to_owned(),
                signature_fingerprint: "sig-py".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert python symbol");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-rust".to_owned(),
                sir_hash: "hash-rust".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1_700_000_100,
            })
            .expect("upsert rust embedding");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-py".to_owned(),
                sir_hash: "hash-py".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![0.0, 1.0],
                updated_at: 1_700_000_101,
            })
            .expect("upsert python embedding");

        let rows = store
            .list_embeddings_for_provider_model("mock", "mock-64d")
            .expect("list embeddings");
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|row| row.symbol_id == "sym-rust" && row.language == "rust")
        );
        assert!(
            rows.iter()
                .any(|row| row.symbol_id == "sym-py" && row.language == "python")
        );
    }

    #[test]
    fn coupling_mining_state_round_trip_persists_latest_values() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        assert!(
            store
                .get_coupling_mining_state()
                .expect("read empty state")
                .is_none()
        );

        store
            .upsert_coupling_mining_state(CouplingMiningStateRecord {
                last_commit_hash: Some("abc123".to_owned()),
                last_mined_at: Some(1_700_000_000_000),
                commits_scanned: 42,
            })
            .expect("upsert state");
        store
            .upsert_coupling_mining_state(CouplingMiningStateRecord {
                last_commit_hash: Some("def456".to_owned()),
                last_mined_at: Some(1_700_000_100_000),
                commits_scanned: 99,
            })
            .expect("upsert updated state");

        let state = store
            .get_coupling_mining_state()
            .expect("read state")
            .expect("state exists");
        assert_eq!(state.last_commit_hash.as_deref(), Some("def456"));
        assert_eq!(state.last_mined_at, Some(1_700_000_100_000));
        assert_eq!(state.commits_scanned, 99);
    }

    #[test]
    fn list_project_notes_for_file_ref_matches_exact_file_ref() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        store
            .upsert_project_note(ProjectNoteRecord {
                note_id: "note-a".to_owned(),
                content: "Store contract for graph schema changes".to_owned(),
                content_hash: "hash-a".to_owned(),
                source_type: "manual".to_owned(),
                source_agent: None,
                tags: vec!["architecture".to_owned()],
                entity_refs: Vec::new(),
                file_refs: vec!["crates/aether-store/src/lib.rs".to_owned()],
                symbol_refs: Vec::new(),
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
                access_count: 0,
                last_accessed_at: None,
                is_archived: false,
            })
            .expect("upsert matching note");
        store
            .upsert_project_note(ProjectNoteRecord {
                note_id: "note-b".to_owned(),
                content: "Unrelated file".to_owned(),
                content_hash: "hash-b".to_owned(),
                source_type: "manual".to_owned(),
                source_agent: None,
                tags: vec!["misc".to_owned()],
                entity_refs: Vec::new(),
                file_refs: vec!["src/main.rs".to_owned()],
                symbol_refs: Vec::new(),
                created_at: 1_700_000_000_001,
                updated_at: 1_700_000_000_001,
                access_count: 0,
                last_accessed_at: None,
                is_archived: false,
            })
            .expect("upsert non-matching note");

        let matches = store
            .list_project_notes_for_file_ref("crates/aether-store/src/lib.rs", 10)
            .expect("query file ref");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].note_id, "note-a");
    }
}
