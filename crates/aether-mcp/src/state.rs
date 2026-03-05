use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, load_workspace_config};
use aether_store::{
    GraphStore, SchemaVersion, SqliteGraphStore, SqliteStore, SqliteVectorStore, SurrealGraphStore,
    VectorStore, open_vector_store,
};
use tokio::sync::Mutex;

use crate::AetherMcpError;

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
}

impl SharedState {
    pub fn open_readwrite(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        let config = Arc::new(load_config(&workspace)?);
        let store = Arc::new(SqliteStore::open(&workspace)?);
        store.check_compatibility("core", 5)?;
        let graph = open_shared_graph(&workspace, &config, false)?;
        let surreal_graph = Arc::new(Mutex::new(None));
        let vector_store = open_vector_store_sync_optional(&workspace, &config)?;
        let schema_version = store.get_schema_version()?;

        Ok(Self {
            workspace,
            store,
            graph,
            surreal_graph,
            vector_store,
            config,
            read_only: false,
            schema_version,
        })
    }

    pub async fn open_readwrite_async(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        let config = Arc::new(load_config(&workspace)?);
        let store = Arc::new(SqliteStore::open(&workspace)?);
        store.check_compatibility("core", 5)?;
        let (graph, surreal_graph) = open_shared_graph_async(&workspace, &config, false).await?;
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
            read_only: false,
            schema_version,
        })
    }

    pub async fn open_readonly_async(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        ensure_workspace_store_ready(&workspace)?;
        let config = Arc::new(load_config(&workspace)?);
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 5)?;
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
        })
    }

    pub fn open_readonly(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        ensure_workspace_store_ready(&workspace)?;
        let config = Arc::new(load_config(&workspace)?);
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 5)?;
        let graph = open_shared_graph(&workspace, &config, true)?;
        let surreal_graph = Arc::new(Mutex::new(None));
        let vector_store = open_vector_store_sync_optional(&workspace, &config)?;
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
        })
    }

    pub fn require_writable(&self) -> Result<(), AetherMcpError> {
        if self.read_only {
            return Err(AetherMcpError::ReadOnly(
                "This AETHER MCP server is read-only.".to_owned(),
            ));
        }
        Ok(())
    }

    pub async fn surreal_graph_for_health(&self) -> Result<Arc<SurrealGraphStore>, AetherMcpError> {
        if self.config.storage.graph_backend != GraphBackend::Surreal {
            return Err(AetherMcpError::Message(
                "health analysis requires surreal graph backend".to_owned(),
            ));
        }

        let mut guard = self.surreal_graph.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }

        let graph = Arc::new(SurrealGraphStore::open_readonly(&self.workspace).await?);
        *guard = Some(graph.clone());
        Ok(graph)
    }
}

fn ensure_workspace_store_ready(workspace: &Path) -> Result<(), AetherMcpError> {
    let aether_dir = workspace.join(".aether");
    std::fs::create_dir_all(&aether_dir)?;

    let sqlite_path = aether_dir.join("meta.sqlite");
    if !sqlite_path.exists() {
        let _bootstrap = SqliteStore::open(workspace)?;
    }

    Ok(())
}

fn load_config(workspace: &Path) -> Result<AetherConfig, AetherMcpError> {
    load_workspace_config(workspace)
        .map_err(|err| AetherMcpError::Message(format!("failed to load workspace config: {err}")))
}

fn open_vector_store_sync_optional(
    workspace: &Path,
    config: &AetherConfig,
) -> Result<Option<Arc<dyn VectorStore>>, AetherMcpError> {
    if !config.embeddings.enabled {
        return Ok(None);
    }

    let vector_store: Arc<dyn VectorStore> = match config.embeddings.vector_backend {
        EmbeddingVectorBackend::Sqlite => Arc::new(SqliteVectorStore::new(workspace)?),
        EmbeddingVectorBackend::Lancedb => {
            if tokio::runtime::Handle::try_current().is_ok() {
                return Err(AetherMcpError::Message(
                    "cannot initialize LanceDB vector store synchronously from an async runtime; use AetherMcpServer::init".to_owned(),
                ));
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    AetherMcpError::Message(format!(
                        "failed to create runtime for vector store initialization: {err}"
                    ))
                })?;
            runtime.block_on(open_vector_store(workspace))?
        }
    };

    Ok(Some(vector_store))
}

async fn open_vector_store_async_optional(
    workspace: &Path,
    config: &AetherConfig,
) -> Result<Option<Arc<dyn VectorStore>>, AetherMcpError> {
    if !config.embeddings.enabled {
        return Ok(None);
    }

    Ok(Some(open_vector_store(workspace).await?))
}

fn open_shared_graph(
    workspace: &Path,
    config: &AetherConfig,
    read_only: bool,
) -> Result<Arc<dyn GraphStore>, AetherMcpError> {
    // MCP handlers only use the `GraphStore` read APIs (callers/dependencies/call-chain).
    // Using the sqlite-backed `GraphStore` avoids Cozo/sled file-lock contention with analyzers
    // that still open their own Cozo handles in the same process.
    let graph: Arc<dyn GraphStore> = match config.storage.graph_backend {
        GraphBackend::Surreal | GraphBackend::Sqlite | GraphBackend::Cozo => {
            if read_only {
                Arc::new(SqliteGraphStore::open_readonly(workspace)?)
            } else {
                Arc::new(SqliteGraphStore::open(workspace)?)
            }
        }
    };
    Ok(graph)
}

async fn open_shared_graph_async(
    workspace: &Path,
    config: &AetherConfig,
    read_only: bool,
) -> Result<(Arc<dyn GraphStore>, Option<Arc<SurrealGraphStore>>), AetherMcpError> {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::SharedState;
    use crate::AetherMcpError;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    #[test]
    fn shared_state_open_readonly_opens_readonly_mode() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let _rw = SharedState::open_readwrite(workspace).expect("open readwrite state");
        let ro = SharedState::open_readonly(workspace).expect("open readonly state");

        assert!(ro.read_only);
        assert_eq!(ro.schema_version.component, "core");
        assert_eq!(ro.schema_version.version, 5);
        assert!(ro.schema_version.migrated_at > 0);
    }

    #[test]
    fn shared_state_require_writable_rejects_readonly() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let _rw = SharedState::open_readwrite(workspace).expect("open readwrite state");
        let ro = SharedState::open_readonly(workspace).expect("open readonly state");

        let err = ro.require_writable().expect_err("readonly must fail");
        match err {
            AetherMcpError::ReadOnly(_) => {}
            other => panic!("expected ReadOnly error, got {other}"),
        }
    }
}
