use std::path::{Path, PathBuf};
use std::sync::Arc;

use aether_config::{
    AetherConfig, DEFAULT_OPENAI_COMPAT_API_KEY_ENV, EmbeddingProviderKind, EmbeddingVectorBackend,
    GraphBackend, load_workspace_config,
};
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
    pub semantic_search_available: bool,
    pub config: Arc<AetherConfig>,
    pub read_only: bool,
    pub schema_version: SchemaVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SemanticSearchUnavailability {
    EmbeddingsDisabled,
    MissingApiKey { key_env: String },
    MissingApiKeyConfig { provider: &'static str },
}

impl SemanticSearchUnavailability {
    pub(crate) fn fallback_reason(&self) -> String {
        match self {
            Self::EmbeddingsDisabled => "embeddings are disabled in .aether/config.toml".to_owned(),
            Self::MissingApiKey { key_env } => format!(
                "Embedding API key not configured. Register MCP server with --env {key_env}=<value> to enable semantic search."
            ),
            Self::MissingApiKeyConfig { provider } => format!(
                "Embedding provider {provider} requires embeddings.api_key_env to be configured before semantic search can be used."
            ),
        }
    }

    fn warn(&self) {
        match self {
            Self::EmbeddingsDisabled => {}
            Self::MissingApiKey { key_env } => tracing::warn!(
                "Embedding provider requires {} but it is not set. Semantic search will be unavailable. Register the MCP server with --env {}=<value> to enable it.",
                key_env,
                key_env
            ),
            Self::MissingApiKeyConfig { provider } => tracing::warn!(
                "Embedding provider {} requires embeddings.api_key_env to be configured. Semantic search will be unavailable.",
                provider
            ),
        }
    }
}

pub(crate) fn semantic_search_unavailability(
    config: &AetherConfig,
) -> Option<SemanticSearchUnavailability> {
    if !config.embeddings.enabled {
        return Some(SemanticSearchUnavailability::EmbeddingsDisabled);
    }

    match config.embeddings.provider {
        EmbeddingProviderKind::Qwen3Local | EmbeddingProviderKind::Candle => None,
        EmbeddingProviderKind::OpenAiCompat => {
            let key_env = config
                .embeddings
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_OPENAI_COMPAT_API_KEY_ENV);
            if env_var_configured(key_env) {
                None
            } else {
                Some(SemanticSearchUnavailability::MissingApiKey {
                    key_env: key_env.to_owned(),
                })
            }
        }
        EmbeddingProviderKind::GeminiNative => {
            let Some(key_env) = config
                .embeddings
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Some(SemanticSearchUnavailability::MissingApiKeyConfig {
                    provider: EmbeddingProviderKind::GeminiNative.as_str(),
                });
            };

            if env_var_configured(key_env) {
                None
            } else {
                Some(SemanticSearchUnavailability::MissingApiKey {
                    key_env: key_env.to_owned(),
                })
            }
        }
    }
}

fn env_var_configured(key_env: &str) -> bool {
    std::env::var(key_env)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

impl SharedState {
    pub fn open_readwrite(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        let config = Arc::new(load_config(&workspace)?);
        let semantic_search_available = match semantic_search_unavailability(config.as_ref()) {
            Some(unavailability) => {
                unavailability.warn();
                false
            }
            None => true,
        };
        let store = Arc::new(SqliteStore::open(&workspace)?);
        store.check_compatibility("core", 18)?;
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
            semantic_search_available,
            config,
            read_only: false,
            schema_version,
        })
    }

    pub async fn open_readwrite_async(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        let config = Arc::new(load_config(&workspace)?);
        let semantic_search_available = match semantic_search_unavailability(config.as_ref()) {
            Some(unavailability) => {
                unavailability.warn();
                false
            }
            None => true,
        };
        let store = Arc::new(SqliteStore::open(&workspace)?);
        store.check_compatibility("core", 18)?;
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
            semantic_search_available,
            config,
            read_only: false,
            schema_version,
        })
    }

    pub async fn open_readonly_async(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        ensure_workspace_store_ready(&workspace)?;
        let config = Arc::new(load_config(&workspace)?);
        let semantic_search_available = match semantic_search_unavailability(config.as_ref()) {
            Some(unavailability) => {
                unavailability.warn();
                false
            }
            None => true,
        };
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 18)?;
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
            semantic_search_available,
            config,
            read_only: true,
            schema_version,
        })
    }

    pub fn open_readonly(workspace: &Path) -> Result<Self, AetherMcpError> {
        let workspace = workspace.canonicalize()?;
        ensure_workspace_store_ready(&workspace)?;
        let config = Arc::new(load_config(&workspace)?);
        let semantic_search_available = match semantic_search_unavailability(config.as_ref()) {
            Some(unavailability) => {
                unavailability.warn();
                false
            }
            None => true,
        };
        let store = Arc::new(SqliteStore::open_readonly(&workspace)?);
        store.check_compatibility("core", 18)?;
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
            semantic_search_available,
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

    pub async fn surreal_graph(&self) -> Result<Arc<SurrealGraphStore>, AetherMcpError> {
        if !matches!(
            self.config.storage.graph_backend,
            GraphBackend::Surreal | GraphBackend::Cozo
        ) {
            return Err(AetherMcpError::Message(
                "operation requires surreal-compatible graph backend".to_owned(),
            ));
        }

        let mut guard = self.surreal_graph.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }

        let graph = Arc::new(if self.read_only {
            SurrealGraphStore::open_readonly(&self.workspace).await?
        } else {
            SurrealGraphStore::open(&self.workspace).await?
        });
        *guard = Some(graph.clone());
        Ok(graph)
    }

    pub async fn surreal_graph_for_health(&self) -> Result<Arc<SurrealGraphStore>, AetherMcpError> {
        self.surreal_graph().await
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
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{SymbolCatalogStore, SymbolRelationStore};
    use rusqlite::{Connection, params};
    use tempfile::tempdir;

    use super::SharedState;
    use crate::AetherMcpError;

    fn write_test_config_with_backend(workspace: &Path, backend: &str) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
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

    fn write_test_config(workspace: &Path) {
        write_test_config_with_backend(workspace, "sqlite");
    }

    fn write_remote_embedding_config(workspace: &Path, api_key_env: &str) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "openai_compat"
vector_backend = "sqlite"
endpoint = "https://example.invalid/v1"
model = "text-embedding-3-large"
api_key_env = "{api_key_env}"
"#
            ),
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
        assert_eq!(ro.schema_version.version, 18);
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

    #[test]
    fn shared_state_marks_semantic_search_unavailable_when_remote_api_key_is_missing() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let env_name = format!(
            "AETHER_TEST_MCP_MISSING_EMBED_KEY_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        );
        write_remote_embedding_config(workspace, &env_name);

        unsafe {
            env::remove_var(&env_name);
        }

        let state = SharedState::open_readwrite(workspace).expect("open readwrite state");
        assert!(!state.semantic_search_available);
    }

    fn seed_symbol_neighbors(workspace: &Path) {
        let store = aether_store::SqliteStore::open(workspace).expect("open sqlite store");
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
    async fn shared_state_async_surreal_backend_uses_sqlite_primary_graph_reads() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config_with_backend(workspace, "surreal");
        seed_symbol_neighbors(workspace);

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
