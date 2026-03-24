use aetherd::enhance::{EnhanceDocumentFormat, EnhanceRequest, EnhanceResult, enhance_prompt_core};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AetherMcpServer;
use crate::AetherMcpError;

const DEFAULT_BUDGET: usize = 8_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherEnhancePromptRequest {
    /// The raw prompt to enhance
    pub prompt: String,
    /// Token budget for context (default: 8000)
    #[serde(default = "default_budget")]
    pub budget: usize,
    /// Whether to use LLM rewrite mode (default: false)
    #[serde(default)]
    pub rewrite: bool,
    /// Output format: "text" or "json" (default: "text")
    #[serde(default = "default_format")]
    pub format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherEnhancePromptResponse {
    pub enhanced_prompt: String,
    pub resolved_symbols: Vec<String>,
    pub referenced_files: Vec<String>,
    pub rewrite_used: bool,
    pub token_count: usize,
    pub warnings: Vec<String>,
}

fn default_budget() -> usize {
    DEFAULT_BUDGET
}

fn default_format() -> String {
    "text".to_owned()
}

fn parse_format(raw: &str) -> Result<EnhanceDocumentFormat, AetherMcpError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "text" => Ok(EnhanceDocumentFormat::Text),
        "json" => Ok(EnhanceDocumentFormat::Json),
        other => Err(AetherMcpError::Message(format!(
            "invalid format '{other}', expected text or json"
        ))),
    }
}

impl From<EnhanceResult> for AetherEnhancePromptResponse {
    fn from(value: EnhanceResult) -> Self {
        Self {
            enhanced_prompt: value.enhanced_prompt,
            resolved_symbols: value.resolved_symbols,
            referenced_files: value.referenced_files,
            rewrite_used: value.rewrite_used,
            token_count: value.token_count,
            warnings: value.warnings,
        }
    }
}

impl AetherMcpServer {
    pub fn aether_enhance_prompt_logic(
        &self,
        request: AetherEnhancePromptRequest,
    ) -> Result<AetherEnhancePromptResponse, AetherMcpError> {
        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            return Err(AetherMcpError::Message(
                "prompt must not be empty".to_owned(),
            ));
        }

        let format = parse_format(request.format.as_str())?;
        let enhance_request =
            EnhanceRequest::new(prompt, request.budget, request.rewrite, false).with_format(format);
        let result = enhance_prompt_core(
            self.workspace(),
            self.state.store.as_ref(),
            &enhance_request,
        )
        .map_err(|err| AetherMcpError::Message(err.to_string()))?;
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::Value;
    use tempfile::tempdir;

    use super::{AetherEnhancePromptRequest, AetherEnhancePromptResponse};
    use crate::AetherMcpServer;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "gemini"
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
    fn request_and_response_serialize() {
        let request = AetherEnhancePromptRequest {
            prompt: "fix auth".to_owned(),
            budget: 16000,
            rewrite: true,
            format: "json".to_owned(),
        };
        let request_value = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(
            request_value.get("prompt").and_then(Value::as_str),
            Some("fix auth")
        );
        assert_eq!(
            request_value.get("budget").and_then(Value::as_u64),
            Some(16000)
        );

        let response = AetherEnhancePromptResponse {
            enhanced_prompt: "prompt".to_owned(),
            resolved_symbols: vec!["demo::auth".to_owned()],
            referenced_files: vec!["src/lib.rs".to_owned()],
            rewrite_used: false,
            token_count: 42,
            warnings: vec!["fallback".to_owned()],
        };
        let response_value = serde_json::to_value(&response).expect("serialize response");
        assert_eq!(
            response_value
                .get("resolved_symbols")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            response_value.get("token_count").and_then(Value::as_u64),
            Some(42)
        );
    }

    #[test]
    fn no_symbols_found_returns_original_prompt_with_warning() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let server = AetherMcpServer::new(workspace, false).expect("server");

        let response = server
            .aether_enhance_prompt_logic(AetherEnhancePromptRequest {
                prompt: "investigate auth".to_owned(),
                budget: 8000,
                rewrite: false,
                format: "text".to_owned(),
            })
            .expect("enhance");

        assert_eq!(response.enhanced_prompt, "investigate auth");
        assert!(
            response
                .warnings
                .iter()
                .any(|warning| warning.contains("contains no symbols"))
        );
    }
}
