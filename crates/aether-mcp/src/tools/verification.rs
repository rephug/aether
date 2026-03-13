use aether_config::VerifyMode;
use aether_core::normalize_path;
use aetherd::verification::{VerificationRequest, run_verification};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MCP_SCHEMA_VERSION};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyRequest {
    pub commands: Option<Vec<String>>,
    pub mode: Option<AetherVerifyMode>,
    pub fallback_to_host_on_unavailable: Option<bool>,
    pub fallback_to_container_on_unavailable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyCommandResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherVerifyMode {
    Host,
    Container,
    Microvm,
}

impl From<AetherVerifyMode> for VerifyMode {
    fn from(value: AetherVerifyMode) -> Self {
        match value {
            AetherVerifyMode::Host => VerifyMode::Host,
            AetherVerifyMode::Container => VerifyMode::Container,
            AetherVerifyMode::Microvm => VerifyMode::Microvm,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherVerifyResponse {
    pub schema_version: u32,
    pub workspace: String,
    pub mode: String,
    pub mode_requested: String,
    pub mode_used: String,
    pub fallback_reason: Option<String>,
    pub allowlisted_commands: Vec<String>,
    pub requested_commands: Vec<String>,
    pub passed: bool,
    pub error: Option<String>,
    pub result_count: u32,
    pub results: Vec<AetherVerifyCommandResult>,
}

impl AetherMcpServer {
    pub fn aether_verify_logic(
        &self,
        request: AetherVerifyRequest,
    ) -> Result<AetherVerifyResponse, AetherMcpError> {
        let execution = run_verification(
            self.workspace(),
            self.state.config.as_ref(),
            VerificationRequest {
                commands: request.commands,
                mode: request.mode.map(Into::into),
                fallback_to_host_on_unavailable: request.fallback_to_host_on_unavailable,
                fallback_to_container_on_unavailable: request.fallback_to_container_on_unavailable,
            },
        )
        .map_err(|err| AetherMcpError::Message(format!("failed to run verification: {err}")))?;

        let results = execution
            .command_results
            .into_iter()
            .map(|item| AetherVerifyCommandResult {
                command: item.command,
                exit_code: item.exit_code,
                stdout: item.stdout,
                stderr: item.stderr,
                passed: item.passed,
            })
            .collect::<Vec<_>>();
        let result_count = results.len() as u32;

        Ok(AetherVerifyResponse {
            schema_version: MCP_SCHEMA_VERSION,
            workspace: normalize_path(&self.state.workspace.to_string_lossy()),
            mode: execution.mode,
            mode_requested: execution.mode_requested,
            mode_used: execution.mode_used,
            fallback_reason: execution.fallback_reason,
            allowlisted_commands: execution.allowlisted_commands,
            requested_commands: execution.requested_commands,
            passed: execution.passed,
            error: execution.error,
            result_count,
            results,
        })
    }
}
