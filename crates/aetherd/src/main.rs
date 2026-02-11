use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::PathBuf;

use aether_config::{InferenceProviderKind, ensure_workspace_config};
use aetherd::indexer::{IndexerConfig, run_indexing_loop};
use aetherd::search::run_search_once;
use aetherd::sir_pipeline::DEFAULT_SIR_CONCURRENCY;
use anyhow::{Context, Result};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "AETHER Observer daemon")]
struct Cli {
    #[arg(long, default_value = ".")]
    workspace: PathBuf,

    #[arg(long, default_value_t = 300)]
    debounce_ms: u64,

    #[arg(long)]
    print_events: bool,

    #[arg(long)]
    print_sir: bool,

    #[arg(long)]
    lsp: bool,

    #[arg(long)]
    index: bool,

    #[arg(long)]
    search: Option<String>,

    #[arg(long, default_value_t = 20)]
    search_limit: u32,

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

    ensure_workspace_config(&workspace).with_context(|| {
        format!(
            "failed to load or create workspace config at {}",
            workspace.join(".aether/config.toml").display()
        )
    })?;

    if let Some(query) = cli.search.as_deref() {
        let mut out = std::io::stdout();
        return run_search_once(&workspace, query, cli.search_limit.min(100), &mut out);
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
                    eprintln!("INDEX: error: {err:#}");
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

    run_indexing_loop(indexer_config)
}

fn parse_inference_provider(value: &str) -> Result<InferenceProviderKind, String> {
    value.parse()
}
