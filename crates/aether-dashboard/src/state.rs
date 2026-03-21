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
        store.check_compatibility("core", 14)?;
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
    match config.storage.graph_backend {
        GraphBackend::Sqlite | GraphBackend::Cozo => {
            let graph: Arc<dyn GraphStore> = if read_only {
                Arc::new(SqliteGraphStore::open_readonly(workspace)?)
            } else {
                Arc::new(SqliteGraphStore::open(workspace)?)
            };
            Ok((graph, None))
        }
        GraphBackend::Surreal => {
            let surreal = if read_only {
                Arc::new(SurrealGraphStore::open_readonly(workspace).await?)
            } else {
                Arc::new(SurrealGraphStore::open(workspace).await?)
            };
            let graph: Arc<dyn GraphStore> = surreal.clone();
            Ok((graph, Some(surreal)))
        }
    }
}
