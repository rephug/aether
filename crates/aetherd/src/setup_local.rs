use std::io::{self, Write};
use std::path::Path;

use aether_config::{
    InferenceProviderKind, OLLAMA_DEFAULT_ENDPOINT, RECOMMENDED_OLLAMA_MODEL,
    ensure_workspace_config, save_workspace_config,
};
use aether_infer::{
    InferenceProvider, OllamaPullProgress, Qwen3LocalProvider, SirContext, fetch_ollama_tags,
    normalize_ollama_endpoint, pull_ollama_model_with_progress,
};
use aether_sir::{SirAnnotation, validate_sir};
use anyhow::{Context, Result, anyhow};
use serde_json::Value;

const SETUP_SMOKE_TEST_SNIPPET: &str = "fn add(a: i32, b: i32) -> i32 { a + b }";
const SETUP_SMOKE_TEST_FILE_PATH: &str = "setup_local_smoke.rs";
const SETUP_SMOKE_TEST_QUALIFIED_NAME: &str = "setup_local::add";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLocalOptions {
    pub endpoint: String,
    pub model: Option<String>,
    pub skip_pull: bool,
    pub skip_config: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupLocalExitCode {
    Success,
    OllamaUnreachable,
    PullFailed,
    TestFailed,
}

impl SetupLocalExitCode {
    pub fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::OllamaUnreachable => 1,
            Self::PullFailed => 2,
            Self::TestFailed => 3,
        }
    }
}

pub fn run_setup_local(workspace: &Path, options: SetupLocalOptions) -> Result<SetupLocalExitCode> {
    let endpoint = normalize_ollama_endpoint(&normalize_non_empty(options.endpoint.as_str()));
    let model = normalize_non_empty_option(options.model)
        .unwrap_or_else(|| RECOMMENDED_OLLAMA_MODEL.to_owned());

    eprintln!("[1/5] Checking Ollama at {endpoint}...");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for setup-local")?;

    let tags_response = match runtime.block_on(fetch_ollama_tags(&endpoint)) {
        Ok(response) => response,
        Err(err) => {
            eprintln!("      ✖ Ollama is unreachable: {err}");
            return Ok(SetupLocalExitCode::OllamaUnreachable);
        }
    };
    eprintln!("      ✔ Ollama API reachable");

    eprintln!("[2/5] Checking for model {model}...");
    let installed_models = match parse_model_names_from_tags(&tags_response) {
        Ok(models) => models,
        Err(err) => {
            eprintln!("      ✖ failed to parse model list: {err}");
            return Ok(SetupLocalExitCode::OllamaUnreachable);
        }
    };
    let model_is_present = installed_models.iter().any(|installed| installed == &model);

    if !model_is_present {
        if options.skip_pull {
            eprintln!("      model not found and --skip-pull enabled; continuing without pull");
        } else {
            eprintln!("[3/5] Pulling model {model}...");
            let pull_result = runtime.block_on(pull_ollama_model_with_progress(
                &endpoint,
                &model,
                print_pull_progress,
            ));
            if let Err(err) = pull_result {
                eprintln!("      ✖ model pull failed: {err}");
                return Ok(SetupLocalExitCode::PullFailed);
            }
            eprintln!("      ✔ model pull complete");
        }
    } else {
        eprintln!("[3/5] Model already present, skipping pull");
    }

    eprintln!("[4/5] Testing SIR generation...");
    let provider = Qwen3LocalProvider::new(Some(endpoint.clone()), Some(model.clone()));
    let smoke_test = runtime.block_on(run_sir_smoke_test(&provider));
    let sir = match smoke_test {
        Ok(sir) => sir,
        Err(err) => {
            eprintln!("      ✖ SIR smoke test failed: {err}");
            return Ok(SetupLocalExitCode::TestFailed);
        }
    };
    eprintln!(
        "      ✔ generated valid SIR (confidence: {:.2})",
        sir.confidence
    );

    if options.skip_config {
        eprintln!("[5/5] Skipping config update (--skip-config)");
    } else {
        eprintln!("[5/5] Updating .aether/config.toml...");
        update_workspace_inference_config(workspace, &endpoint, &model)?;
        eprintln!("      ✔ set provider=qwen3_local model={model}");
    }

    eprintln!("Local inference is ready.");
    Ok(SetupLocalExitCode::Success)
}

fn normalize_non_empty(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        OLLAMA_DEFAULT_ENDPOINT.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn normalize_non_empty_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_model_names_from_tags(value: &Value) -> Result<Vec<String>> {
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("missing models array in /api/tags response"))?;

    let mut names = Vec::new();
    for model in models {
        let Some(name) = model.get("name").and_then(Value::as_str) else {
            continue;
        };
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            names.push(trimmed.to_owned());
        }
    }

    names.sort();
    names.dedup();
    Ok(names)
}

fn print_pull_progress(progress: OllamaPullProgress) {
    if let (Some(completed), Some(total)) = (progress.completed, progress.total)
        && total > 0
    {
        let percent = ((completed as f64 / total as f64) * 100.0).clamp(0.0, 100.0);
        eprint!("\r      {percent:6.2}%  {}", progress.status);
    } else {
        eprintln!("      {}", progress.status);
    }
    let _ = io::stderr().flush();

    if progress.done {
        eprintln!();
    }
}

async fn run_sir_smoke_test(provider: &dyn InferenceProvider) -> Result<SirAnnotation> {
    let context = SirContext {
        language: "rust".to_owned(),
        file_path: SETUP_SMOKE_TEST_FILE_PATH.to_owned(),
        qualified_name: SETUP_SMOKE_TEST_QUALIFIED_NAME.to_owned(),
    };
    let sir = provider
        .generate_sir(SETUP_SMOKE_TEST_SNIPPET, &context)
        .await
        .context("failed to generate SIR from smoke-test snippet")?;
    validate_smoke_test_sir(&sir)?;
    Ok(sir)
}

fn validate_smoke_test_sir(sir: &SirAnnotation) -> Result<()> {
    validate_sir(sir).context("generated SIR failed schema validation")
}

fn update_workspace_inference_config(workspace: &Path, endpoint: &str, model: &str) -> Result<()> {
    let mut config = ensure_workspace_config(workspace)
        .with_context(|| format!("failed to load workspace config at {}", workspace.display()))?;
    config.inference.provider = InferenceProviderKind::Qwen3Local;
    config.inference.model = Some(model.to_owned());
    config.inference.endpoint = Some(endpoint.to_owned());
    save_workspace_config(workspace, &config)
        .with_context(|| format!("failed to save workspace config at {}", workspace.display()))
}

#[cfg(test)]
mod tests {
    use aether_config::load_workspace_config;
    use aether_sir::SirAnnotation;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        parse_model_names_from_tags, update_workspace_inference_config, validate_smoke_test_sir,
    };

    #[test]
    fn updates_workspace_config_for_qwen_local_provider() {
        let temp = tempdir().expect("tempdir");
        update_workspace_inference_config(
            temp.path(),
            "http://127.0.0.1:11434",
            "qwen2.5-coder:7b-instruct-q4_K_M",
        )
        .expect("update config");

        let config = load_workspace_config(temp.path()).expect("load config");
        assert_eq!(config.inference.provider.as_str(), "qwen3_local");
        assert_eq!(
            config.inference.model.as_deref(),
            Some("qwen2.5-coder:7b-instruct-q4_K_M")
        );
        assert_eq!(
            config.inference.endpoint.as_deref(),
            Some("http://127.0.0.1:11434")
        );
    }

    #[test]
    fn parses_model_names_from_tags_payload() {
        let tags = json!({
            "models": [
                {"name": "qwen2.5-coder:7b-instruct-q4_K_M"},
                {"name": "mistral:7b"},
                {"digest": "missing-name"}
            ]
        });

        let models = parse_model_names_from_tags(&tags).expect("parse models");
        assert_eq!(
            models,
            vec![
                "mistral:7b".to_owned(),
                "qwen2.5-coder:7b-instruct-q4_K_M".to_owned()
            ]
        );
    }

    #[test]
    fn validates_smoke_test_sir_output() {
        let valid = SirAnnotation {
            intent: "Adds two numbers".to_owned(),
            inputs: vec!["a".to_owned(), "b".to_owned()],
            outputs: vec!["sum".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.8,
        };
        validate_smoke_test_sir(&valid).expect("valid sir");

        let invalid = SirAnnotation {
            intent: "   ".to_owned(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.8,
        };
        let err = validate_smoke_test_sir(&invalid).expect_err("expected invalid sir");
        assert!(
            err.to_string()
                .contains("generated SIR failed schema validation")
        );
        assert_eq!(err.root_cause().to_string(), "intent is required");
    }
}
