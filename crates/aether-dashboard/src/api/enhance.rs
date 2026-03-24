use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::support::{self, DashboardState};

const DEFAULT_BUDGET: usize = 8_000;
const ENHANCE_TIMEOUT_SECS: u64 = 30;
const ENHANCE_TIMEOUT_MESSAGE: &str =
    "Prompt enhancement timed out. Try disabling rewrite or lowering the budget.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EnhanceApiRequest {
    pub prompt: String,
    #[serde(default = "default_budget")]
    pub budget: usize,
    #[serde(default)]
    pub rewrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct EnhanceApiResponse {
    pub enhanced_prompt: String,
    pub resolved_symbols: Vec<String>,
    pub referenced_files: Vec<String>,
    pub rewrite_used: bool,
    pub token_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnhanceApiError {
    Timeout(String),
    Internal(String),
}

fn default_budget() -> usize {
    DEFAULT_BUDGET
}

pub(crate) async fn enhance_handler(
    State(state): State<Arc<DashboardState>>,
    Json(payload): Json<EnhanceApiRequest>,
) -> Response {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return json_bad_request("prompt must not be empty");
    }
    if payload.budget == 0 {
        return json_bad_request("budget must be greater than 0");
    }

    let request = EnhanceApiRequest {
        prompt: prompt.to_owned(),
        budget: payload.budget,
        rewrite: payload.rewrite,
    };

    match run_enhance_request(
        state.shared.workspace.clone(),
        request,
        execute_enhance_subprocess,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(EnhanceApiError::Timeout(message)) => support::json_timeout_error(message),
        Err(EnhanceApiError::Internal(message)) => support::json_internal_error(message),
    }
}

async fn run_enhance_request<F>(
    workspace: PathBuf,
    request: EnhanceApiRequest,
    runner: F,
) -> Result<EnhanceApiResponse, EnhanceApiError>
where
    F: FnOnce(&Path, &EnhanceApiRequest) -> Result<EnhanceApiResponse, String> + Send + 'static,
{
    let join = tokio::time::timeout(
        Duration::from_secs(ENHANCE_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || runner(workspace.as_path(), &request)),
    )
    .await
    .map_err(|_| EnhanceApiError::Timeout(ENHANCE_TIMEOUT_MESSAGE.to_owned()))?;

    match join {
        Ok(result) => result.map_err(EnhanceApiError::Internal),
        Err(err) => Err(EnhanceApiError::Internal(format!(
            "dashboard task join failure: {err}"
        ))),
    }
}

fn execute_enhance_subprocess(
    workspace: &Path,
    request: &EnhanceApiRequest,
) -> Result<EnhanceApiResponse, String> {
    let executable = std::env::current_exe()
        .map_err(|err| format!("failed to locate current executable: {err}"))?;

    let mut command = Command::new(&executable);
    command
        .arg("--workspace")
        .arg(workspace)
        .arg("enhance")
        .arg(request.prompt.as_str())
        .arg("--output")
        .arg("json")
        .arg("--budget")
        .arg(request.budget.to_string());
    if request.rewrite {
        command.arg("--rewrite");
    }

    let output = command
        .output()
        .map_err(|err| format!("failed to execute {}: {err}", executable.display()))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_owned()
        } else {
            stderr.trim().to_owned()
        };
        let exit = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_owned());
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(format!(
            "enhance command failed with exit code {exit}{suffix}"
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("enhance command produced non-UTF-8 stdout: {err}"))?;
    parse_enhance_response(stdout.as_str())
}

fn parse_enhance_response(stdout: &str) -> Result<EnhanceApiResponse, String> {
    serde_json::from_str(stdout.trim())
        .map_err(|err| format!("failed to parse enhance JSON output: {err}"))
}

fn json_bad_request(message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": "bad_request",
            "message": message.into(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        ENHANCE_TIMEOUT_MESSAGE, ENHANCE_TIMEOUT_SECS, EnhanceApiError, EnhanceApiRequest,
        EnhanceApiResponse, parse_enhance_response, run_enhance_request,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn request_and_response_round_trip() {
        let request = EnhanceApiRequest {
            prompt: "fix auth".to_owned(),
            budget: 9_000,
            rewrite: true,
        };
        let request_json = serde_json::to_string(&request).expect("serialize request");
        let restored: EnhanceApiRequest =
            serde_json::from_str(request_json.as_str()).expect("deserialize request");
        assert_eq!(restored.prompt, "fix auth");
        assert_eq!(restored.budget, 9_000);
        assert!(restored.rewrite);

        let response = EnhanceApiResponse {
            enhanced_prompt: "## Enhanced Prompt".to_owned(),
            resolved_symbols: vec!["demo::auth".to_owned()],
            referenced_files: vec!["src/auth.rs".to_owned()],
            rewrite_used: false,
            token_count: 42,
            warnings: vec!["fallback".to_owned()],
        };
        let response_json = serde_json::to_string(&response).expect("serialize response");
        let restored: EnhanceApiResponse =
            serde_json::from_str(response_json.as_str()).expect("deserialize response");
        assert_eq!(restored.resolved_symbols.len(), 1);
        assert_eq!(restored.token_count, 42);
    }

    #[test]
    fn parse_enhance_response_rejects_invalid_json() {
        let err = parse_enhance_response("not json").expect_err("parse should fail");
        assert!(err.contains("failed to parse enhance JSON output"));
    }

    #[tokio::test]
    async fn run_enhance_request_returns_runner_response() {
        let request = EnhanceApiRequest {
            prompt: "fix auth".to_owned(),
            budget: 8_000,
            rewrite: false,
        };

        let response = run_enhance_request(
            PathBuf::from("/tmp/workspace"),
            request,
            |_workspace, request| {
                Ok(EnhanceApiResponse {
                    enhanced_prompt: format!("enhanced {}", request.prompt),
                    resolved_symbols: vec!["demo::auth".to_owned()],
                    referenced_files: vec!["src/auth.rs".to_owned()],
                    rewrite_used: request.rewrite,
                    token_count: 128,
                    warnings: Vec::new(),
                })
            },
        )
        .await
        .expect("runner should succeed");

        assert_eq!(response.enhanced_prompt, "enhanced fix auth");
        assert_eq!(response.resolved_symbols, vec!["demo::auth".to_owned()]);
    }

    #[tokio::test]
    async fn run_enhance_request_times_out() {
        let request = EnhanceApiRequest {
            prompt: "fix auth".to_owned(),
            budget: 8_000,
            rewrite: false,
        };

        let err = run_enhance_request(
            PathBuf::from("/tmp/workspace"),
            request,
            |_workspace, _request| {
                std::thread::sleep(Duration::from_secs(ENHANCE_TIMEOUT_SECS + 1));
                Ok(EnhanceApiResponse {
                    enhanced_prompt: String::new(),
                    resolved_symbols: Vec::new(),
                    referenced_files: Vec::new(),
                    rewrite_used: false,
                    token_count: 0,
                    warnings: Vec::new(),
                })
            },
        )
        .await
        .expect_err("runner should time out");

        assert_eq!(
            err,
            EnhanceApiError::Timeout(ENHANCE_TIMEOUT_MESSAGE.to_owned())
        );
    }
}
