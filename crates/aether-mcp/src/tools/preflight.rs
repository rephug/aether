use aetherd::preflight::{PreflightRequest, assemble_preflight_measured};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AetherMcpServer;
use crate::AetherMcpError;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct AetherPreflightRequest {
    /// Task-specific problem or change to investigate.
    pub task: String,
    /// Optional workspace-relative source path; no branch is required.
    pub scope: Option<String>,
    /// Serialized MCP tool-result UTF-8 bytes / 4 estimate; default 2500.
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherPreflightResponse {
    /// Shared CLI/MCP map with context, provenance, omissions and size estimate.
    pub map: serde_json::Value,
}

impl AetherMcpServer {
    pub fn aether_preflight_logic(
        &self,
        request: AetherPreflightRequest,
    ) -> Result<AetherPreflightResponse, AetherMcpError> {
        let request = PreflightRequest {
            task: request.task,
            scope: request.scope,
            max_tokens: request.max_tokens.unwrap_or(2500),
        };
        let map = assemble_preflight_measured(self.workspace(), &request, |map| {
            let response = AetherPreflightResponse {
                map: serde_json::to_value(map)?,
            };
            let result = rmcp::model::CallToolResult::structured(serde_json::to_value(response)?);
            Ok(serde_json::to_vec(&result)?.len().div_ceil(4))
        })
        .map_err(|error| {
            AetherMcpError::Message(format!("pre-flight assembly failed: {error:#}"))
        })?;
        Ok(AetherPreflightResponse {
            map: serde_json::to_value(map)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_store::SqliteStore;
    use rmcp::handler::server::tool::IntoCallToolResult;
    use std::sync::Arc;

    #[tokio::test]
    async fn preflight_uses_shared_service_and_budgets_actual_mcp_result() -> anyhow::Result<()> {
        let workspace = tempfile::tempdir()?;
        std::fs::write(workspace.path().join("cache.rs"), "fn cache() {}")?;
        let _writer = SqliteStore::open(workspace.path())?;
        let state = Arc::new(crate::SharedState::open_readonly(workspace.path())?);
        let server = AetherMcpServer::from_state(state, false);
        let request = AetherPreflightRequest {
            task: "repair cache".to_owned(),
            scope: None,
            max_tokens: Some(2500),
        };
        let response = server.aether_preflight_logic(request)?;
        assert!(
            response.map["context"]
                .as_str()
                .unwrap_or_default()
                .contains("cache.rs")
        );
        let estimate = response.map["estimated_tokens"]
            .as_u64()
            .unwrap_or_default();
        let result = rmcp::Json(response).into_call_tool_result()?;
        assert!(serde_json::to_vec(&result)?.len().div_ceil(4) as u64 <= estimate);
        assert!(estimate <= 2500);
        assert!(!workspace.path().join(".aether/graph").exists());
        Ok(())
    }
}
