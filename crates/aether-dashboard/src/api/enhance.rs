use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::process::Command;

use crate::support::{self, DashboardState};

const DEFAULT_BUDGET: usize = 8_000;
const ENHANCE_TIMEOUT_SECS: u64 = 30;
const ENHANCE_TIMEOUT_MESSAGE: &str =
    "Prompt enhancement timed out. Try disabling rewrite or lowering the budget.";
const AETHERD_BINARY_NAME: &str = "aetherd";
const AETHERD_UNAVAILABLE_MESSAGE: &str = "Prompt enhancement requires the aetherd binary, \
     which was not found next to this executable or on PATH.";

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

    let current_exe = std::env::current_exe().ok();
    let path_var = std::env::var_os("PATH");
    let Some(executable) = resolve_aetherd_executable(current_exe.as_deref(), path_var.as_deref())
    else {
        return json_service_unavailable(AETHERD_UNAVAILABLE_MESSAGE);
    };

    match run_enhance_request(
        state.shared.workspace.clone(),
        request,
        move |workspace, request| execute_enhance_subprocess(executable, workspace, request),
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(EnhanceApiError::Timeout(message)) => support::json_timeout_error(message),
        Err(EnhanceApiError::Internal(message)) => support::json_internal_error(message),
    }
}

async fn run_enhance_request<F, Fut>(
    workspace: PathBuf,
    request: EnhanceApiRequest,
    runner: F,
) -> Result<EnhanceApiResponse, EnhanceApiError>
where
    F: FnOnce(PathBuf, EnhanceApiRequest) -> Fut,
    Fut: Future<Output = Result<EnhanceApiResponse, String>>,
{
    tokio::time::timeout(
        Duration::from_secs(ENHANCE_TIMEOUT_SECS),
        runner(workspace, request),
    )
    .await
    .map_err(|_| EnhanceApiError::Timeout(ENHANCE_TIMEOUT_MESSAGE.to_owned()))?
    .map_err(EnhanceApiError::Internal)
}

/// Finds the `aetherd` binary that runs `enhance`. The dashboard is also served by
/// other hosts (for example the desktop app), so the current executable is only
/// reused when it is `aetherd` itself; otherwise a sibling `aetherd`, then PATH.
fn resolve_aetherd_executable(
    current_exe: Option<&Path>,
    path_var: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(current) = current_exe
        && current.file_stem() == Some(OsStr::new(AETHERD_BINARY_NAME))
    {
        return Some(current.to_path_buf());
    }
    let file_name = format!("{AETHERD_BINARY_NAME}{}", std::env::consts::EXE_SUFFIX);
    let sibling_dir = current_exe.and_then(Path::parent).map(Path::to_path_buf);
    let path_dirs = path_var.into_iter().flat_map(std::env::split_paths);
    sibling_dir
        .into_iter()
        .chain(path_dirs)
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
}

fn build_enhance_command(
    executable: &Path,
    workspace: &Path,
    request: &EnhanceApiRequest,
) -> Command {
    let mut command = Command::new(executable);
    command
        .kill_on_drop(true)
        // stdout must carry only the JSON document.
        .env("RUST_LOG", "off")
        .arg("--workspace")
        .arg(workspace)
        .arg("enhance")
        .arg("--output")
        .arg("json")
        .arg("--budget")
        .arg(request.budget.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if request.rewrite {
        command.arg("--rewrite");
    }
    // `--` keeps prompts that start with '-' from being parsed as flags.
    command.arg("--").arg(&request.prompt);
    command
}

async fn execute_enhance_subprocess(
    executable: PathBuf,
    workspace: PathBuf,
    request: EnhanceApiRequest,
) -> Result<EnhanceApiResponse, String> {
    let mut command = build_enhance_command(&executable, &workspace, &request);

    let child = command
        .spawn()
        .map_err(|err| format!("failed to execute {}: {err}", executable.display()))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|err| format!("failed to wait for {}: {err}", executable.display()))?;

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

/// Parses the enhance JSON, tolerating stray non-JSON lines (such as log output)
/// before or after the document.
fn parse_enhance_response(stdout: &str) -> Result<EnhanceApiResponse, String> {
    let trimmed = stdout.trim();
    serde_json::from_str(trimmed).or_else(|err| {
        trimmed
            .match_indices('{')
            .find_map(|(start, _)| {
                serde_json::Deserializer::from_str(&trimmed[start..])
                    .into_iter::<EnhanceApiResponse>()
                    .next()
                    .and_then(Result::ok)
            })
            .ok_or_else(|| format!("failed to parse enhance JSON output: {err}"))
    })
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

fn json_service_unavailable(message: impl Into<String>) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "service_unavailable",
            "message": message.into(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{
        ENHANCE_TIMEOUT_MESSAGE, ENHANCE_TIMEOUT_SECS, EnhanceApiError, EnhanceApiRequest,
        EnhanceApiResponse, build_enhance_command, parse_enhance_response,
        resolve_aetherd_executable, run_enhance_request,
    };
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
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

    #[test]
    fn parse_enhance_response_skips_log_lines_around_json() {
        let stdout = "\u{1b}[2m2026-01-01T00:00:00Z\u{1b}[0m \u{1b}[33m WARN\u{1b}[0m failed to parse SIR blob {symbol_id=\"abc\"}\n\
             {\n  \"enhanced_prompt\": \"fix {auth}\",\n  \"resolved_symbols\": [\"login\"],\n  \
             \"referenced_files\": [\"src/lib.rs\"],\n  \"rewrite_used\": false,\n  \
             \"token_count\": 7,\n  \"warnings\": []\n}\n\
             INFO trailing log line\n";

        let response = parse_enhance_response(stdout).expect("log lines should be skipped");

        assert_eq!(response.enhanced_prompt, "fix {auth}");
        assert_eq!(response.resolved_symbols, vec!["login".to_owned()]);
        assert_eq!(response.token_count, 7);
    }

    #[test]
    fn enhance_command_passes_hyphen_prompt_after_separator_with_logging_off() {
        let request = EnhanceApiRequest {
            prompt: "--help me fix the -x flag".to_owned(),
            budget: 9_000,
            rewrite: true,
        };

        let command = build_enhance_command(
            Path::new("/opt/aether/aetherd"),
            Path::new("/tmp/workspace"),
            &request,
        );
        let command = command.as_std();

        let args: Vec<&OsStr> = command.get_args().collect();
        let expected: Vec<&OsStr> = [
            "--workspace",
            "/tmp/workspace",
            "enhance",
            "--output",
            "json",
            "--budget",
            "9000",
            "--rewrite",
            "--",
            "--help me fix the -x flag",
        ]
        .into_iter()
        .map(OsStr::new)
        .collect();
        assert_eq!(args, expected);
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "RUST_LOG" && value == Some(OsStr::new("off")))
        );
    }

    #[test]
    fn resolve_aetherd_never_reexecutes_a_non_aetherd_host() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_dir = temp.path().join("host");
        let path_dir = temp.path().join("bin");
        std::fs::create_dir_all(&host_dir).expect("create host dir");
        std::fs::create_dir_all(&path_dir).expect("create path dir");
        let aetherd_name = format!("aetherd{}", std::env::consts::EXE_SUFFIX);
        let desktop = host_dir.join("aether-desktop");
        std::fs::write(&desktop, b"").expect("write host binary");
        let path_var: OsString =
            std::env::join_paths([path_dir.as_path()]).expect("join PATH entries");

        // A host that is not aetherd, with no aetherd anywhere, resolves to nothing.
        assert_eq!(
            resolve_aetherd_executable(Some(&desktop), Some(&path_var)),
            None
        );

        // aetherd on PATH is used.
        let on_path = path_dir.join(&aetherd_name);
        std::fs::write(&on_path, b"").expect("write PATH aetherd");
        assert_eq!(
            resolve_aetherd_executable(Some(&desktop), Some(&path_var)),
            Some(on_path)
        );

        // A sibling aetherd wins over PATH.
        let sibling = host_dir.join(&aetherd_name);
        std::fs::write(&sibling, b"").expect("write sibling aetherd");
        assert_eq!(
            resolve_aetherd_executable(Some(&desktop), Some(&path_var)),
            Some(sibling)
        );

        // When the host is aetherd itself, it is reused.
        let aetherd_host = temp.path().join("elsewhere").join(&aetherd_name);
        assert_eq!(
            resolve_aetherd_executable(Some(&aetherd_host), None),
            Some(aetherd_host.clone())
        );
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
            |_workspace, request| async move {
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
        struct DropGuard {
            dropped: Arc<AtomicBool>,
        }

        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::SeqCst);
            }
        }

        let request = EnhanceApiRequest {
            prompt: "fix auth".to_owned(),
            budget: 8_000,
            rewrite: false,
        };
        let dropped = Arc::new(AtomicBool::new(false));

        let err = run_enhance_request(PathBuf::from("/tmp/workspace"), request, {
            let dropped = Arc::clone(&dropped);
            move |_workspace, _request| async move {
                let _guard = DropGuard { dropped };
                tokio::time::sleep(Duration::from_secs(ENHANCE_TIMEOUT_SECS + 1)).await;
                Ok(EnhanceApiResponse {
                    enhanced_prompt: String::new(),
                    resolved_symbols: Vec::new(),
                    referenced_files: Vec::new(),
                    rewrite_used: false,
                    token_count: 0,
                    warnings: Vec::new(),
                })
            }
        })
        .await
        .expect_err("runner should time out");

        assert_eq!(
            err,
            EnhanceApiError::Timeout(ENHANCE_TIMEOUT_MESSAGE.to_owned())
        );
        assert!(dropped.load(Ordering::SeqCst));
    }
}
