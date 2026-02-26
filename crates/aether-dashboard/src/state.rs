use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, load_workspace_config};
use aether_store::{
    GraphStore, SchemaVersion, SqliteGraphStore, SqliteStore, SqliteVectorStore, SurrealGraphStore,
    VectorStore, open_vector_store,
};

pub type DashboardStateError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct SharedState {
    pub workspace: PathBuf,
    pub store: Arc<SqliteStore>,
    pub graph: Arc<dyn GraphStore>,
    pub vector_store: Option<Arc<dyn VectorStore>>,
    pub config: Arc<AetherConfig>,
    pub read_only: bool,
    pub schema_version: SchemaVersion,
}

impl SharedState {
    pub async fn open_readonly_async(workspace: &Path) -> Result<Self, DashboardStateError> {
        let workspace = workspace.canonicalize()?;
        let config = Arc::new(load_workspace_config(&workspace)?);
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 2)?;
        let graph = open_shared_graph_async(&workspace, &config, true).await?;
        let vector_store = open_vector_store_async_optional(&workspace, &config).await?;
        let schema_version = store.get_schema_version()?;

        Ok(Self {
            workspace,
            store,
            graph,
            vector_store,
            config,
            read_only: true,
            schema_version,
        })
    }
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
) -> Result<Arc<dyn GraphStore>, DashboardStateError> {
    let graph: Arc<dyn GraphStore> = match config.storage.graph_backend {
        GraphBackend::Sqlite | GraphBackend::Cozo => {
            if read_only {
                Arc::new(SqliteGraphStore::open_readonly(workspace)?)
            } else {
                Arc::new(SqliteGraphStore::open(workspace)?)
            }
        }
        GraphBackend::Surreal => {
            if read_only {
                Arc::new(SurrealGraphStore::open_readonly(workspace).await?)
            } else {
                Arc::new(SurrealGraphStore::open(workspace).await?)
            }
        }
    };
    Ok(graph)
}
