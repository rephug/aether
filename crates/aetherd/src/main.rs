use std::io::IsTerminal;
use std::path::Path;

use aether_config::{
    DEFAULT_LOG_LEVEL, SearchRerankerKind, ensure_workspace_config, validate_config,
};
use aether_infer::{download_candle_embedding_model, download_candle_reranker_model};
use aetherd::calibrate::run_calibration_once;
use aetherd::cli::{Cli, Commands, InitAgentArgs, LogFormat, SetupLocalArgs, parse_cli};
use aetherd::indexer::{IndexerConfig, run_indexing_loop, run_initial_index_once};
use aetherd::init_agent::{InitAgentOptions, run_init_agent};
use aetherd::search::run_search_once;
use aetherd::setup_local::{SetupLocalOptions, run_setup_local};
use aetherd::verification::{VerificationRequest, run_verification};
use anyhow::{Context, Result, anyhow};

fn main() -> Result<()> {
    let cli = parse_cli();
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    let workspace = cli.workspace.canonicalize().with_context(|| {
        format!(
            "failed to resolve workspace path {}",
            cli.workspace.display()
        )
    })?;

    if let Some(command) = cli.command.clone() {
        return run_subcommand(&workspace, command);
    }

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
        let model_root = download_candle_embedding_model(&workspace, cli.model_dir.clone())
            .context("failed to download Candle embedding model files")?;
        tracing::info!(
            model_root = %model_root.display(),
            "downloaded Candle embedding model files"
        );

        if matches!(config.search.reranker, SearchRerankerKind::Candle) {
            let reranker_model_root = download_candle_reranker_model(&workspace, cli.model_dir)
                .context("failed to download Candle reranker model files")?;
            tracing::info!(
                model_root = %reranker_model_root.display(),
                "downloaded Candle reranker model files"
            );
        }

        return Ok(());
    }

    if cli.calibrate {
        return run_calibration_once(&workspace).context("failed to calibrate search thresholds");
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

fn run_subcommand(workspace: &Path, command: Commands) -> Result<()> {
    match command {
        Commands::InitAgent(args) => run_init_agent_command(workspace, args),
        Commands::SetupLocal(args) => run_setup_local_command(workspace, args),
    }
}

fn run_init_agent_command(workspace: &Path, args: InitAgentArgs) -> Result<()> {
    let outcome = run_init_agent(
        workspace,
        InitAgentOptions {
            platform: args.platform,
            force: args.force,
        },
    )
    .context("failed to generate agent integration files")?;

    if outcome.used_default_config {
        eprintln!(
            "warning: missing .aether/config.toml, generated templates using default config values"
        );
    }

    for path in &outcome.written_files {
        eprintln!("generated {}", path.display());
    }

    if outcome.exit_code() == 2 {
        for path in &outcome.skipped_existing_files {
            eprintln!(
                "skipped existing {} (re-run with --force to overwrite)",
                path.display()
            );
        }
        std::process::exit(2);
    }

    Ok(())
}

fn run_setup_local_command(workspace: &Path, args: SetupLocalArgs) -> Result<()> {
    let exit_code = run_setup_local(
        workspace,
        SetupLocalOptions {
            endpoint: args.endpoint,
            model: args.model,
            skip_pull: args.skip_pull,
            skip_config: args.skip_config,
        },
    )
    .context("setup-local failed")?;

    if exit_code.code() != 0 {
        std::process::exit(exit_code.code());
    }

    Ok(())
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
