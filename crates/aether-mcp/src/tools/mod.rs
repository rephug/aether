use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_sir::{SirAnnotation, validate_sir};
use aether_store::{SirMetaRecord, Store};
use anyhow::Result;
use rmcp::ServiceExt;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::transport::stdio;

use crate::AetherMcpError;
use crate::state::SharedState;

mod common;
mod drift;
mod health;
mod history;
mod impact;
mod memory;
mod router;
mod search;
mod sir;
mod status;
#[cfg(feature = "verification")]
mod verification;

pub use drift::*;
pub use health::*;
pub use history::*;
pub use impact::*;
pub use memory::*;
pub use search::*;
pub use sir::*;
pub use status::*;
#[cfg(feature = "verification")]
pub use verification::*;

pub(crate) use common::{effective_limit, symbol_leaf_name};

pub const SERVER_NAME: &str = "aether";
pub const SERVER_VERSION: &str = "0.1.0";
pub const SERVER_DESCRIPTION: &str = "AETHER local symbol/SIR lookup from .aether store";
pub const MCP_SCHEMA_VERSION: u32 = 1;
pub const MEMORY_SCHEMA_VERSION: &str = "1.0";
pub(crate) const SIR_STATUS_GENERATING: &str = "generating";

#[derive(Clone)]
pub struct AetherMcpServer {
    pub(crate) state: Arc<SharedState>,
    pub(crate) verbose: bool,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl AetherMcpServer {
    pub fn new(workspace: impl AsRef<Path>, verbose: bool) -> Result<Self, AetherMcpError> {
        let state = Arc::new(SharedState::open_readwrite(workspace.as_ref())?);
        Ok(Self::from_state(state, verbose))
    }

    pub async fn init(workspace: impl AsRef<Path>, verbose: bool) -> Result<Self, AetherMcpError> {
        let state = Arc::new(SharedState::open_readwrite_async(workspace.as_ref()).await?);
        Ok(Self::from_state(state, verbose))
    }

    pub fn from_state(state: Arc<SharedState>, verbose: bool) -> Self {
        let tool_router = Self::tool_router();
        #[cfg(feature = "verification")]
        let tool_router =
            tool_router.with_route((Self::aether_verify_tool_attr(), Self::aether_verify));

        Self {
            state,
            verbose,
            tool_router,
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.state.workspace
    }

    pub(crate) fn sqlite_path(&self) -> PathBuf {
        self.state.workspace.join(".aether").join("meta.sqlite")
    }

    pub(crate) fn sir_dir(&self) -> PathBuf {
        self.state.workspace.join(".aether").join("sir")
    }

    pub(crate) fn resolve_workspace_file_path(
        &self,
        file_path: &str,
    ) -> Result<PathBuf, AetherMcpError> {
        let path = PathBuf::from(file_path);
        let joined = if path.is_absolute() {
            path
        } else {
            self.state.workspace.join(path)
        };

        let absolute = joined.canonicalize()?;
        if !absolute.starts_with(self.workspace()) {
            return Err(AetherMcpError::Message(format!(
                "file_path must be under workspace {}",
                self.workspace().display()
            )));
        }

        Ok(absolute)
    }

    pub(crate) fn workspace_relative_display_path(&self, absolute_path: &Path) -> String {
        if let Ok(relative) = absolute_path.strip_prefix(self.workspace()) {
            return aether_core::normalize_path(&relative.to_string_lossy());
        }

        aether_core::normalize_path(&absolute_path.to_string_lossy())
    }

    pub(crate) fn read_valid_sir_blob(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirAnnotation>, AetherMcpError> {
        if !self.sqlite_path().exists() {
            return Ok(None);
        }

        let store = self.state.store.as_ref();
        let blob = store.read_sir_blob(symbol_id)?;

        let Some(blob) = blob else {
            return Ok(None);
        };

        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;
        Ok(Some(sir))
    }

    pub(crate) fn read_sir_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirMetaRecord>, AetherMcpError> {
        if !self.sqlite_path().exists() {
            return Ok(None);
        }

        let store = self.state.store.as_ref();
        store.get_sir_meta(symbol_id).map_err(Into::into)
    }

    pub(crate) fn verbose_log(&self, message: &str) {
        if self.verbose {
            tracing::debug!(message = %message, "aether-mcp verbose");
        }
    }
}

pub async fn run_stdio_server(workspace: impl AsRef<Path>, verbose: bool) -> Result<()> {
    let server = AetherMcpServer::init(workspace, verbose).await?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub(crate) fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub(crate) fn current_unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
