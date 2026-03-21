use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, load_workspace_config};
use aether_health::ScoreReport;
use aether_store::{
    GraphStore, SchemaVersion, SqliteGraphStore, SqliteStore, SqliteVectorStore, SurrealGraphStore,
    VectorStore, open_vector_store,
};

use crate::narrative::LayerAssignmentsCache;

pub type DashboardStateError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Default)]
pub struct DashboardCaches {
    pub project_summary: Mutex<Option<(i64, String)>>,
    pub layer_assignments: Mutex<Option<(i64, LayerAssignmentsCache)>>,
    pub health_score_report: RwLock<Option<(Instant, ScoreReport)>>,
}

#[derive(Clone)]
pub struct SharedState {
    pub workspace: PathBuf,
    pub store: Arc<SqliteStore>,
    pub graph: Arc<dyn GraphStore>,
    pub surreal_graph: Arc<Mutex<Option<Arc<SurrealGraphStore>>>>,
    pub vector_store: Option<Arc<dyn VectorStore>>,
    pub config: Arc<AetherConfig>,
    pub read_only: bool,
    pub schema_version: SchemaVersion,
    pub caches: Arc<DashboardCaches>,
}

impl SharedState {
    pub async fn open_readonly_async(workspace: &Path) -> Result<Self, DashboardStateError> {
        let workspace = workspace.canonicalize()?;
        ensure_workspace_store_ready(&workspace)?;
        let config = Arc::new(load_workspace_config(&workspace)?);
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 15)?;
        let (graph, surreal_graph) = open_shared_graph_async(&workspace, &config, true).await?;
        let surreal_graph = Arc::new(Mutex::new(surreal_graph));
        let vector_store = open_vector_store_async_optional(&workspace, &config).await?;
        let schema_version = store.get_schema_version()?;

        Ok(Self {
            workspace,
            store,
            graph,
            surreal_graph,
            vector_store,
            config,
            read_only: true,
            schema_version,
            caches: Arc::new(DashboardCaches::default()),
        })
    }

    pub async fn surreal_graph_store(&self) -> Result<Arc<SurrealGraphStore>, DashboardStateError> {
        if self.config.storage.graph_backend != GraphBackend::Surreal {
            return Err(std::io::Error::other(
                "dashboard health/coupling operations require surreal graph backend",
            )
            .into());
        }

        {
            let guard = self
                .surreal_graph
                .lock()
                .map_err(|e| std::io::Error::other(format!("surreal graph lock poisoned: {e}")))?;
            if let Some(existing) = guard.as_ref() {
                return Ok(existing.clone());
            }
        }

        let graph = Arc::new(SurrealGraphStore::open_readonly(&self.workspace).await?);

        let mut guard = self
            .surreal_graph
            .lock()
            .map_err(|e| std::io::Error::other(format!("surreal graph lock poisoned: {e}")))?;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }
        *guard = Some(graph.clone());
        Ok(graph)
    }
}

fn ensure_workspace_store_ready(workspace: &Path) -> Result<(), DashboardStateError> {
    let aether_dir = workspace.join(".aether");
    std::fs::create_dir_all(&aether_dir)?;

    let sqlite_path = aether_dir.join("meta.sqlite");
    if !sqlite_path.exists() {
        let _bootstrap = SqliteStore::open(workspace)?;
    }

    Ok(())
}

async fn open_vector_store_async_optional(
    workspace: &Path,
    config: &AetherConfig,
) -> Result<Option<Arc<dyn VectorStore>>, DashboardStateError> {
    if !config.embeddings.enabled {
        return Ok(None);
    }

    match config.embeddings.vector_backend {
        EmbeddingVectorBackend::Sqlite => Ok(Some(Arc::new(SqliteVectorStore::new(workspace)?))),
        EmbeddingVectorBackend::Lancedb => Ok(Some(open_vector_store(workspace).await?)),
    }
}

async fn open_shared_graph_async(
    workspace: &Path,
    config: &AetherConfig,
    read_only: bool,
) -> Result<(Arc<dyn GraphStore>, Option<Arc<SurrealGraphStore>>), DashboardStateError> {
    let graph: Arc<dyn GraphStore> = if read_only {
        Arc::new(SqliteGraphStore::open_readonly(workspace)?)
    } else {
        Arc::new(SqliteGraphStore::open(workspace)?)
    };

    let surreal = match config.storage.graph_backend {
        GraphBackend::Surreal => {
            match if read_only {
                SurrealGraphStore::open_readonly(workspace).await
            } else {
                SurrealGraphStore::open(workspace).await
            } {
                Ok(store) => Some(Arc::new(store)),
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "SurrealDB graph unavailable (daemon may hold lock), using SQLite only"
                    );
                    None
                }
            }
        }
        GraphBackend::Sqlite | GraphBackend::Cozo => None,
    };

    Ok((graph, surreal))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRelationStore};
    use rusqlite::{Connection, params};
    use tempfile::tempdir;

    use super::SharedState;

    fn write_test_config(workspace: &Path, backend: &str) {
        std::fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        std::fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "{backend}"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#
            ),
        )
        .expect("write config");
    }

    fn seed_sqlite_neighbors(workspace: &Path) {
        let store = SqliteStore::open(workspace).expect("open sqlite store");
        let alpha = aether_store::SymbolRecord {
            id: "sym-alpha".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "alpha".to_owned(),
            signature_fingerprint: "sig-alpha".to_owned(),
            last_seen_at: 1,
        };
        let beta = aether_store::SymbolRecord {
            id: "sym-beta".to_owned(),
            file_path: "src/dep.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "beta".to_owned(),
            signature_fingerprint: "sig-beta".to_owned(),
            last_seen_at: 2,
        };
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store.upsert_symbol(beta.clone()).expect("upsert beta");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: alpha.id.clone(),
                target_qualified_name: beta.qualified_name.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: alpha.file_path.clone(),
            }])
            .expect("upsert edge");

        let conn = Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite db");
        conn.execute(
            r#"
            INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![alpha.id, beta.id, "calls", beta.qualified_name, beta.file_path],
        )
        .expect("insert forward neighbor");
        conn.execute(
            r#"
            INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params!["sym-beta", "sym-alpha", "called_by", "alpha", "src/lib.rs"],
        )
        .expect("insert reverse neighbor");
    }

    #[tokio::test]
    async fn surreal_backend_uses_sqlite_primary_graph_reads() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace, "surreal");
        seed_sqlite_neighbors(workspace);

        let state = SharedState::open_readonly_async(workspace)
            .await
            .expect("open readonly state");

        let callers = state.graph.get_callers("beta").await.expect("load callers");
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].id, "sym-alpha");

        let dependencies = state
            .graph
            .get_dependencies("sym-alpha")
            .await
            .expect("load dependencies");
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].id, "sym-beta");
    }
}
