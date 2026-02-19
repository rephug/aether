use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::PathBuf;

use aether_config::{
    DEFAULT_LOG_LEVEL, InferenceProviderKind, VerifyMode, ensure_workspace_config, validate_config,
};
use aether_infer::download_candle_embedding_model;
use aetherd::indexer::{IndexerConfig, run_indexing_loop, run_initial_index_once};
use aetherd::search::{SearchMode, SearchOutputFormat, run_search_once};
use aetherd::sir_pipeline::DEFAULT_SIR_CONCURRENCY;
use aetherd::verification::{VerificationRequest, run_verification};
use anyhow::{Context, Result, anyhow};
use clap::Parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LogFormat {
    #[default]
    Human,
    Json,
}

impl LogFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid log format '{other}', expected one of: human, json"
            )),
        }
    }
}

#[derive(Debug, Parser)]
#[command(author, version, about = "AETHER Observer daemon")]
struct Cli {
    #[arg(long, default_value = ".", help = "Workspace root to index/search")]
    workspace: PathBuf,

    #[arg(
        long,
        default_value_t = 300,
        help = "Debounce window for watcher events"
    )]
    debounce_ms: u64,

    #[arg(long, help = "Print symbol-change events as JSON lines")]
    print_events: bool,

    #[arg(long, help = "Print SIR lifecycle lines as symbols are processed")]
    print_sir: bool,

    #[arg(
        long,
        default_value = "human",
        value_parser = parse_log_format,
        help = "Log format: human or json"
    )]
    log_format: LogFormat,

    #[arg(long, help = "Run as stdio LSP server")]
    lsp: bool,

    #[arg(
        long,
        requires = "lsp",
        help = "Run background indexing while LSP is active"
    )]
    index: bool,

    #[arg(
        long,
        conflicts_with_all = ["lsp", "index", "index_once", "verify"],
        help = "Run one-shot symbol search and exit"
    )]
    search: Option<String>,

    #[arg(
        long,
        default_value_t = 20,
        requires = "search",
        help = "Result limit for --search (clamped to 1..100)"
    )]
    search_limit: u32,

    #[arg(
        long,
        default_value = "lexical",
        value_parser = parse_search_mode,
        requires = "search",
        help = "Search mode: lexical, semantic, or hybrid. Semantic/hybrid fall back to lexical with a reason when unavailable"
    )]
    search_mode: SearchMode,

    #[arg(
        long,
        default_value = "table",
        value_parser = parse_search_output_format,
        requires = "search",
        help = "Search output format: table or json"
    )]
    output: SearchOutputFormat,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "verify"],
        help = "Run one full index pass and exit"
    )]
    index_once: bool,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "index_once"],
        help = "Run verification commands and exit"
    )]
    verify: bool,

    #[arg(
        long,
        conflicts_with_all = ["search", "lsp", "index", "index_once", "verify"],
        help = "Download local model files required for Candle embeddings and exit"
    )]
    download_models: bool,

    #[arg(
        long,
        requires = "download_models",
        help = "Override model cache directory for --download-models"
    )]
    model_dir: Option<PathBuf>,

    #[arg(
        long,
        requires = "verify",
        help = "Run only the provided allowlisted command"
    )]
    verify_command: Vec<String>,

    #[arg(
        long,
        requires = "verify",
        value_parser = parse_verify_mode,
        help = "Verification mode override: host, container, or microvm"
    )]
    verify_mode: Option<VerifyMode>,

    #[arg(
        long,
        requires = "verify",
        help = "Fall back to host mode when the selected verification runtime is unavailable"
    )]
    verify_fallback_host_on_unavailable: bool,

    #[arg(
        long,
        requires = "verify",
        help = "When verify mode is microvm, fall back to container mode if microvm runtime is unavailable"
    )]
    verify_fallback_container_on_unavailable: bool,

    #[arg(long, default_value_t = DEFAULT_SIR_CONCURRENCY)]
    sir_concurrency: usize,

    #[arg(long, value_parser = parse_inference_provider)]
    inference_provider: Option<InferenceProviderKind>,

    #[arg(long)]
    inference_model: Option<String>,

    #[arg(long)]
    inference_endpoint: Option<String>,

    #[arg(long)]
    inference_api_key_env: Option<String>,
}

fn main() -> Result<()> {
    let cli = parse_cli();
    run(cli)
}

fn parse_cli() -> Cli {
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == OsStr::new("--")) {
        args.remove(1);
    }

    Cli::parse_from(args)
}

fn run(cli: Cli) -> Result<()> {
    let workspace = cli.workspace.canonicalize().with_context(|| {
        format!(
            "failed to resolve workspace path {}",
            cli.workspace.display()
        )
    })?;

    let config = ensure_workspace_config(&workspace).with_context(|| {
        format!(
            "failed to load or create workspace config at {}",
            workspace.join(".aether/config.toml").display()
        )
    })?;
    init_tracing_subscriber(cli.log_format, &config.general.log_level)?;

    for warning in validate_config(&config) {
        tracing::warn!(
            code = warning.code,
            message = %warning.message,
            "AETHER config warning"
        );
    }

    if cli.download_models {
        let model_root = download_candle_embedding_model(&workspace, cli.model_dir)
            .context("failed to download Candle embedding model files")?;
        tracing::info!(
            model_root = %model_root.display(),
            "downloaded Candle embedding model files"
        );
        return Ok(());
    }

    if let Some(query) = cli.search.as_deref() {
        let mut out = std::io::stdout();
        return run_search_once(
            &workspace,
            query,
            cli.search_limit.min(100),
            cli.search_mode,
            cli.output,
            &mut out,
        );
    }

    if cli.verify {
        let requested_commands = (!cli.verify_command.is_empty()).then_some(cli.verify_command);
        let execution = run_verification(
            &workspace,
            &config,
            VerificationRequest {
                commands: requested_commands,
                mode: cli.verify_mode,
                fallback_to_host_on_unavailable: cli
                    .verify_fallback_host_on_unavailable
                    .then_some(true),
                fallback_to_container_on_unavailable: cli
                    .verify_fallback_container_on_unavailable
                    .then_some(true),
            },
        )
        .context("verification execution failed")?;

        if let Some(error) = &execution.error {
            tracing::error!(error = %error, "verification reported error");
        }
        if let Some(reason) = &execution.fallback_reason {
            tracing::warn!(fallback_reason = %reason, "verification fallback");
        }

        tracing::info!(
            mode_requested = %execution.mode_requested,
            mode_used = %execution.mode_used,
            "verification summary"
        );

        for result in &execution.command_results {
            tracing::info!(command = %result.command, "verification command starting");

            if !result.stdout.trim().is_empty() {
                print!("{}", result.stdout);
            }
            if !result.stderr.trim().is_empty() {
                eprint!("{}", result.stderr);
            }

            let exit = result
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_owned());
            if result.passed {
                tracing::info!(
                    command = %result.command,
                    exit_code = %exit,
                    passed = result.passed,
                    "verification command finished"
                );
            } else {
                tracing::warn!(
                    command = %result.command,
                    exit_code = %exit,
                    passed = result.passed,
                    "verification command failed"
                );
            }
        }

        if execution.passed {
            return Ok(());
        }

        let exit_code = execution
            .command_results
            .last()
            .and_then(|result| result.exit_code)
            .filter(|code| *code > 0)
            .unwrap_or(1);
        std::process::exit(exit_code);
    }

    let indexer_config = IndexerConfig {
        workspace: workspace.clone(),
        debounce_ms: cli.debounce_ms,
        print_events: cli.print_events,
        print_sir: cli.print_sir,
        sir_concurrency: cli.sir_concurrency,
        lifecycle_logs: cli.lsp && cli.index && std::io::stdout().is_terminal(),
        inference_provider: cli.inference_provider,
        inference_model: cli.inference_model,
        inference_endpoint: cli.inference_endpoint,
        inference_api_key_env: cli.inference_api_key_env,
    };

    if cli.lsp {
        if cli.index {
            std::thread::spawn(move || {
                if let Err(err) = run_indexing_loop(indexer_config) {
                    tracing::error!(error = %err, "index loop error");
                }
            });
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("failed to build tokio runtime for LSP")?;
        runtime
            .block_on(aether_lsp::run_stdio(workspace))
            .context("LSP server exited with error")?;
        return Ok(());
    }

    if cli.index_once {
        return run_initial_index_once(&indexer_config);
    }

    run_indexing_loop(indexer_config)
}

fn parse_inference_provider(value: &str) -> Result<InferenceProviderKind, String> {
    value.parse()
}

fn parse_search_mode(value: &str) -> Result<SearchMode, String> {
    value.parse()
}

fn parse_search_output_format(value: &str) -> Result<SearchOutputFormat, String> {
    value.parse()
}

fn parse_verify_mode(value: &str) -> Result<VerifyMode, String> {
    value.parse()
}

fn parse_log_format(value: &str) -> Result<LogFormat, String> {
    value.parse()
}

fn init_tracing_subscriber(log_format: LogFormat, configured_log_level: &str) -> Result<()> {
    let init_result = match log_format {
        LogFormat::Human => tracing_subscriber::fmt()
            .with_env_filter(build_env_filter(configured_log_level))
            .with_target(false)
            .try_init(),
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(build_env_filter(configured_log_level))
            .with_target(false)
            .with_current_span(false)
            .with_span_list(false)
            .try_init(),
    };

    init_result.map_err(|err| {
        anyhow!(
            "failed to initialize tracing subscriber (format={}): {err}",
            log_format.as_str()
        )
    })
}

fn build_env_filter(configured_log_level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(configured_log_level))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_LEVEL))
}
