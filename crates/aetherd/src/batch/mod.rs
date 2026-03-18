pub mod hash;

pub(crate) mod anthropic;
mod build;
mod extract;
pub(crate) mod gemini;
mod ingest;
pub(crate) mod openai;
mod run;

use std::path::{Path, PathBuf};

use aether_config::{AetherConfig, BatchConfig};
use anyhow::{Result, anyhow};

use crate::cli::{BatchBuildArgs, BatchPass, BatchRunArgs};

pub(crate) use build::build_pass_jsonl_for_ids;
pub(crate) use ingest::{ingest_results, write_fingerprint_row};
pub use run::run_batch_command;

// ---------------------------------------------------------------------------
// BatchProvider trait + supporting types
// ---------------------------------------------------------------------------

/// Result from parsing one line of batch output.
pub(crate) enum BatchResultLine {
    Success {
        key: String,
        text: String,
    },
    Error {
        key: Option<String>,
        message: String,
    },
}

/// Batch job completion status.
pub(crate) enum BatchPollStatus {
    InProgress {
        completed: Option<u64>,
        total: Option<u64>,
    },
    Completed,
    Failed {
        message: String,
    },
}

/// Abstraction over provider-specific batch API mechanics.
#[async_trait::async_trait]
pub(crate) trait BatchProvider: Send + Sync {
    /// Format one batch request line.
    ///
    /// * `key` — `"symbol_id|prompt_hash"`
    /// * `system_prompt` — static SIR instruction (cacheable)
    /// * `user_prompt` — per-symbol content
    /// * `model` — provider-specific model string
    /// * `thinking` — raw thinking level string ("off", "low", "medium", "high", "dynamic")
    fn format_request(
        &self,
        key: &str,
        system_prompt: &str,
        user_prompt: &str,
        model: &str,
        thinking: &str,
    ) -> Result<String>;

    /// Submit batch input. Returns one or more job IDs.
    async fn submit(
        &self,
        input_path: &Path,
        model: &str,
        batch_dir: &Path,
        poll_interval_secs: u64,
    ) -> Result<Vec<String>>;

    /// Poll jobs for completion.
    async fn poll(&self, job_ids: &[String]) -> Result<BatchPollStatus>;

    /// Download results to output_dir. Returns paths to result files.
    async fn download_results(&self, job_ids: &[String], output_dir: &Path)
    -> Result<Vec<PathBuf>>;

    /// Parse one result line.
    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine>;

    /// Provider name for logging.
    fn name(&self) -> &str;
}

/// Create a batch provider from config with optional CLI override.
pub(crate) fn create_batch_provider(
    config: &BatchConfig,
    provider_override: Option<&str>,
) -> Result<Box<dyn BatchProvider>> {
    let provider_name = provider_override
        .filter(|s| !s.is_empty())
        .unwrap_or(config.provider.as_str());
    let api_key_env = config.resolve_api_key_env(provider_name);
    let api_key = std::env::var(&api_key_env).map_err(|_| {
        anyhow!(
            "batch provider '{}' requires env var {} to be set",
            provider_name,
            api_key_env
        )
    })?;
    match provider_name {
        "gemini" => Ok(Box::new(gemini::GeminiBatchProvider::new(api_key))),
        "openai" => Ok(Box::new(openai::OpenAiBatchProvider::new(api_key))),
        "anthropic" => Ok(Box::new(anthropic::AnthropicBatchProvider::new(api_key))),
        other => anyhow::bail!(
            "unknown batch provider '{}'; supported: gemini, openai, anthropic",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// PassConfig / BatchRuntimeConfig (existing types)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct PassConfig {
    pub pass: BatchPass,
    pub model: String,
    pub thinking: String,
    pub neighbor_depth: u32,
    pub max_chars: usize,
    pub prompt_tier: String,
}

impl PassConfig {
    /// Fingerprint used for prompt hash deduplication. Includes provider to prevent
    /// false matches when switching providers with the same model name.
    pub(crate) fn config_fingerprint(&self, provider: &str) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            provider, self.model, self.thinking, self.max_chars, self.prompt_tier
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BatchRuntimeConfig {
    pub batch_dir: PathBuf,
    pub jsonl_chunk_size: usize,
    pub poll_interval_secs: u64,
    pub scan: PassConfig,
    pub triage: PassConfig,
    pub deep: PassConfig,
}

impl BatchRuntimeConfig {
    pub(crate) fn for_pass(&self, pass: BatchPass) -> &PassConfig {
        match pass {
            BatchPass::Scan => &self.scan,
            BatchPass::Triage => &self.triage,
            BatchPass::Deep => &self.deep,
        }
    }
}

pub(crate) fn resolve_batch_runtime_config(
    workspace: &Path,
    config: &AetherConfig,
    run_args: Option<&BatchRunArgs>,
) -> BatchRuntimeConfig {
    let batch_config = config.batch.as_ref();
    let batch_dir = resolve_batch_dir(
        workspace,
        batch_config,
        run_args.and_then(|args| args.batch_dir.as_deref()),
    );
    BatchRuntimeConfig {
        batch_dir,
        jsonl_chunk_size: run_args
            .and_then(|args| args.jsonl_chunk_size)
            .or_else(|| batch_config.map(|value| value.jsonl_chunk_size))
            .unwrap_or(5_000)
            .max(1),
        poll_interval_secs: run_args
            .and_then(|args| args.poll_interval_secs)
            .or_else(|| batch_config.map(|value| value.poll_interval_secs))
            .unwrap_or(60)
            .max(1),
        scan: resolve_pass_config(batch_config, run_args, BatchPass::Scan),
        triage: resolve_pass_config(batch_config, run_args, BatchPass::Triage),
        deep: resolve_pass_config(batch_config, run_args, BatchPass::Deep),
    }
}

pub(crate) fn resolve_build_pass_config(
    runtime: &BatchRuntimeConfig,
    args: &BatchBuildArgs,
) -> PassConfig {
    let base = runtime.for_pass(args.pass).clone();
    PassConfig {
        pass: args.pass,
        model: args.model.clone().unwrap_or(base.model),
        thinking: args.thinking.clone().unwrap_or(base.thinking),
        neighbor_depth: args.neighbor_depth.unwrap_or(base.neighbor_depth),
        max_chars: args.max_chars.unwrap_or(base.max_chars),
        prompt_tier: base.prompt_tier,
    }
}

pub(crate) fn parse_batch_passes_csv(raw: &str) -> Result<Vec<BatchPass>> {
    let mut passes = Vec::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let pass = match value {
            "scan" => BatchPass::Scan,
            "triage" => BatchPass::Triage,
            "deep" => BatchPass::Deep,
            other => {
                return Err(anyhow!(
                    "invalid batch pass '{other}', expected one of: scan, triage, deep"
                ));
            }
        };
        if !passes.contains(&pass) {
            passes.push(pass);
        }
    }
    if passes.is_empty() {
        return Err(anyhow!("at least one batch pass is required"));
    }
    Ok(passes)
}

fn resolve_batch_dir(
    workspace: &Path,
    batch_config: Option<&BatchConfig>,
    override_dir: Option<&str>,
) -> PathBuf {
    let configured = override_dir
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            batch_config
                .map(|value| value.batch_dir.trim())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| workspace.join(".aether/batch"));
    if configured.is_absolute() {
        configured
    } else {
        workspace.join(configured)
    }
}

fn resolve_pass_config(
    batch_config: Option<&BatchConfig>,
    run_args: Option<&BatchRunArgs>,
    pass: BatchPass,
) -> PassConfig {
    let (model, thinking, neighbor_depth, max_chars) = match pass {
        BatchPass::Scan => (
            run_args.and_then(|args| args.scan_model.clone()),
            run_args.and_then(|args| args.scan_thinking.clone()),
            None,
            run_args.and_then(|args| args.scan_max_chars),
        ),
        BatchPass::Triage => (
            run_args.and_then(|args| args.triage_model.clone()),
            run_args.and_then(|args| args.triage_thinking.clone()),
            run_args.and_then(|args| args.triage_neighbor_depth),
            run_args.and_then(|args| args.triage_max_chars),
        ),
        BatchPass::Deep => (
            run_args.and_then(|args| args.deep_model.clone()),
            run_args.and_then(|args| args.deep_thinking.clone()),
            run_args.and_then(|args| args.deep_neighbor_depth),
            run_args.and_then(|args| args.deep_max_chars),
        ),
    };

    let config = batch_config.cloned().unwrap_or_default();
    match pass {
        BatchPass::Scan => PassConfig {
            pass,
            model: model.unwrap_or(config.scan_model),
            thinking: thinking.unwrap_or(config.scan_thinking),
            neighbor_depth: 0,
            max_chars: max_chars.unwrap_or(config.scan_max_chars),
            prompt_tier: config.prompt_tier.clone(),
        },
        BatchPass::Triage => PassConfig {
            pass,
            model: model.unwrap_or(config.triage_model),
            thinking: thinking.unwrap_or(config.triage_thinking),
            neighbor_depth: neighbor_depth.unwrap_or(config.triage_neighbor_depth),
            max_chars: max_chars.unwrap_or(config.triage_max_chars),
            prompt_tier: config.prompt_tier.clone(),
        },
        BatchPass::Deep => PassConfig {
            pass,
            model: model.unwrap_or(config.deep_model),
            thinking: thinking.unwrap_or(config.deep_thinking),
            neighbor_depth: neighbor_depth.unwrap_or(config.deep_neighbor_depth),
            max_chars: max_chars.unwrap_or(config.deep_max_chars),
            prompt_tier: config.prompt_tier,
        },
    }
}
