use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

use aether_config::{
    AetherConfig, DEFAULT_LOG_LEVEL, InferenceProviderKind, SearchRerankerKind,
    ensure_workspace_config, load_workspace_config, validate_config,
};
use aether_core::{Symbol, SymbolChangeEvent};
use aether_infer::ProviderOverrides;
use aether_infer::sir_prompt::SirEnrichmentContext;
use aether_infer::{download_candle_embedding_model, download_candle_reranker_model};
use aether_sir::{FileSir, SirAnnotation, synthetic_file_sir_id};
#[cfg(feature = "legacy-cozo")]
use aether_store::migrate_cozo_to_surreal;
use aether_store::{SqliteStore, Store};
use aetherd::calibrate::run_calibration_once;
use aetherd::causal::run_trace_cause_command;
use aetherd::cli::{
    AskArgs, BlastRadiusArgs, Cli, Commands, CommunitiesArgs, CouplingReportArgs, DriftAckArgs,
    DriftReportArgs, FsckArgs, HealthArgs, HealthScoreArgs, InitAgentArgs, LogFormat,
    MineCouplingArgs, NotesArgs, RecallArgs, RegenerateArgs, RememberArgs, SetupLocalArgs,
    TestIntentsArgs, TraceCauseArgs, parse_cli,
};
use aetherd::coupling::{
    run_blast_radius_command, run_coupling_report_command, run_mine_coupling_command,
};
use aetherd::drift::{run_communities_command, run_drift_ack_command, run_drift_report_command};
use aetherd::fsck::run_fsck;
use aetherd::health::run_health_command;
use aetherd::health_score::run_health_score_command;
use aetherd::indexer::{
    IndexerConfig, compute_symbol_priority_scores, run_indexing_loop,
    run_initial_index_once_for_cli,
};
use aetherd::init_agent::{InitAgentOptions, run_init_agent};
use aetherd::memory::{
    run_ask_command, run_notes_command, run_recall_command, run_remember_command,
};
use aetherd::observer::ObserverState;
use aetherd::search::run_search_once;
use aetherd::setup_local::{SetupLocalOptions, run_setup_local};
use aetherd::test_intents::run_test_intents_command;
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
    let command = cli.command.clone();
    let config = load_config_for_command(&workspace, command.as_ref())?;
    init_tracing_subscriber(cli.log_format, &config.general.log_level)?;

    if let Some(command) = command {
        return run_subcommand(&workspace, &config, command);
    }

    for warning in validate_config(&config) {
        tracing::warn!(
            code = warning.code,
            message = %warning.message,
            "AETHER config warning"
        );
    }

    if cli.deep && !cli.full {
        return Err(anyhow!(
            "--deep requires --full (quality pipeline needs scan + triage before deep)"
        ));
    }

    let selected_provider = cli.inference_provider.unwrap_or(config.inference.provider);
    let run_triage = config.sir_quality.triage_pass || cli.deep;
    let run_deep = config.sir_quality.deep_pass || cli.deep;
    if run_triage
        && selected_provider == InferenceProviderKind::Qwen3Local
        && config.sir_quality.triage_provider.is_none()
    {
        tracing::info!("Triage pass will use local CoT mode (thinking enabled, 8192 context).");
    }
    if run_deep
        && selected_provider == InferenceProviderKind::Qwen3Local
        && config.sir_quality.deep_provider.is_none()
    {
        tracing::info!("Deep pass will use local CoT mode (thinking enabled, 8192 context).");
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

    #[cfg(feature = "dashboard")]
    {
        let dashboard_enabled = config.dashboard.enabled && !cli.no_dashboard;
        if dashboard_enabled {
            let ws = workspace.clone();
            let dash_port = config.dashboard.port;
            std::thread::spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        tracing::error!(error = %err, "dashboard: failed to build tokio runtime");
                        return;
                    }
                };

                rt.block_on(async move {
                    let state = match aether_dashboard::SharedState::open_readonly_async(&ws).await {
                        Ok(s) => std::sync::Arc::new(s),
                        Err(err) => {
                            tracing::error!(error = %err, "failed to open dashboard state");
                            return;
                        }
                    };
                    let router = aether_dashboard::dashboard_router(state);
                    let bind_addr = format!("127.0.0.1:{dash_port}");
                    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
                        Ok(l) => l,
                        Err(err) => {
                            tracing::error!(error = %err, addr = %bind_addr, "dashboard: failed to bind");
                            return;
                        }
                    };
                    tracing::info!("Dashboard available at http://{bind_addr}/dashboard/");
                    if let Err(err) = axum::serve(listener, router.into_make_service()).await {
                        tracing::error!(error = %err, "dashboard server error");
                    }
                });
            });
        }
    }

    let indexer_config = IndexerConfig {
        workspace: workspace.clone(),
        debounce_ms: cli.debounce_ms,
        print_events: cli.print_events,
        print_sir: cli.print_sir,
        embeddings_only: cli.embeddings_only,
        force: cli.force,
        full: cli.full,
        deep: cli.deep,
        dry_run: cli.dry_run,
        sir_concurrency: cli
            .sir_concurrency
            .unwrap_or(config.inference.concurrency)
            .max(1),
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
        let result = run_initial_index_once_for_cli(&indexer_config);
        match result {
            Ok(()) => std::process::exit(0),
            Err(err) => return Err(err),
        }
    }

    run_indexing_loop(indexer_config)
}

fn run_subcommand(workspace: &Path, config: &AetherConfig, command: Commands) -> Result<()> {
    match command {
        Commands::InitAgent(args) => run_init_agent_command(workspace, args),
        Commands::Regenerate(args) => run_regenerate_command(workspace, args),
        Commands::SetupLocal(args) => run_setup_local_command(workspace, args),
        Commands::Status => run_status_subcommand(workspace),
        Commands::Remember(args) => run_remember_note_command(workspace, args),
        Commands::Recall(args) => run_recall_note_command(workspace, args),
        Commands::Ask(args) => run_ask_subcommand(workspace, args),
        Commands::Notes(args) => run_notes_list_command(workspace, args),
        Commands::MineCoupling(args) => run_mine_coupling_subcommand(workspace, args),
        Commands::BlastRadius(args) => run_blast_radius_subcommand(workspace, args),
        Commands::CouplingReport(args) => run_coupling_report_subcommand(workspace, args),
        Commands::TestIntents(args) => run_test_intents_subcommand(workspace, args),
        Commands::DriftReport(args) => run_drift_report_subcommand(workspace, args),
        Commands::DriftAck(args) => run_drift_ack_subcommand(workspace, args),
        Commands::Communities(args) => run_communities_subcommand(workspace, args),
        Commands::TraceCause(args) => run_trace_cause_subcommand(workspace, args),
        Commands::Health(args) => run_health_subcommand(workspace, args),
        Commands::HealthScore(args) => run_health_score_subcommand(workspace, config, args),
        Commands::Fsck(args) => run_fsck_subcommand(workspace, args),
        #[cfg(feature = "legacy-cozo")]
        Commands::GraphMigrate(args) => run_graph_migrate_subcommand(workspace, args),
    }
}

fn load_config_for_command(workspace: &Path, command: Option<&Commands>) -> Result<AetherConfig> {
    let load_result = match command {
        Some(Commands::HealthScore(_)) => load_workspace_config(workspace).with_context(|| {
            format!(
                "failed to load workspace config at {}",
                workspace.join(".aether/config.toml").display()
            )
        }),
        _ => ensure_workspace_config(workspace).with_context(|| {
            format!(
                "failed to load or create workspace config at {}",
                workspace.join(".aether/config.toml").display()
            )
        }),
    }?;
    Ok(load_result)
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

#[derive(Debug, Clone)]
struct RegenerateCandidate {
    symbol: Symbol,
    provider: String,
    confidence: f32,
    priority: f64,
    baseline_sir: SirAnnotation,
}

fn run_regenerate_command(workspace: &Path, args: RegenerateArgs) -> Result<()> {
    let config = ensure_workspace_config(workspace)
        .context("failed to load workspace config for regenerate command")?;
    let store = SqliteStore::open(workspace).context("failed to open local store")?;

    let mut observer =
        ObserverState::new(workspace.to_path_buf()).context("failed to initialize observer")?;
    observer
        .seed_from_disk()
        .context("failed to seed observer from workspace")?;
    let mut symbols_by_id = HashMap::<String, Symbol>::new();
    for event in observer.initial_symbol_events() {
        for symbol in event.added.iter().chain(event.updated.iter()) {
            symbols_by_id.insert(symbol.id.clone(), symbol.clone());
        }
    }
    let all_symbols = symbols_by_id.values().cloned().collect::<Vec<_>>();
    let priority_scores = compute_symbol_priority_scores(workspace, &store, &all_symbols);

    let mut candidates = Vec::<RegenerateCandidate>::new();
    for symbol in all_symbols {
        if let Some(file_filter) = args.file.as_deref()
            && symbol.file_path != file_filter
        {
            continue;
        }

        let Some(meta) = store.get_sir_meta(symbol.id.as_str())? else {
            continue;
        };
        if let Some(provider_filter) = args.from_provider.as_deref()
            && meta.provider != provider_filter
        {
            continue;
        }

        let Some(blob) = store.read_sir_blob(symbol.id.as_str())? else {
            continue;
        };
        let Ok(sir) = serde_json::from_str::<SirAnnotation>(&blob) else {
            continue;
        };
        if sir.confidence >= args.below_confidence {
            continue;
        }

        candidates.push(RegenerateCandidate {
            symbol,
            provider: meta.provider,
            confidence: sir.confidence,
            priority: priority_scores
                .get(meta.id.as_str())
                .copied()
                .unwrap_or(0.0),
            baseline_sir: sir,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .priority
            .total_cmp(&left.priority)
            .then_with(|| left.confidence.total_cmp(&right.confidence))
            .then_with(|| left.symbol.id.cmp(&right.symbol.id))
    });
    if let Some(limit) = args.max
        && candidates.len() > limit
    {
        candidates.truncate(limit);
    }

    if args.dry_run {
        println!(
            "{:<32} {:<15} {:<10} {:<8}",
            "Symbol", "Provider", "Confidence", "Priority"
        );
        for candidate in &candidates {
            println!(
                "{:<32} {:<15} {:<10.2} {:<8.2}",
                truncate_display_name(candidate.symbol.qualified_name.as_str(), 32),
                truncate_display_name(candidate.provider.as_str(), 15),
                candidate.confidence,
                candidate.priority,
            );
        }
        println!("({} symbols would be regenerated)", candidates.len());
        return Ok(());
    }

    let main_pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        config.inference.concurrency.max(1),
        ProviderOverrides::default(),
    )
    .context("failed to initialize primary regeneration pipeline")?;

    let mut owned_deep_pipeline: Option<SirPipeline> = None;
    if args.deep {
        let deep_provider = config
            .sir_quality
            .deep_provider
            .clone()
            .map(|provider_raw| {
                provider_raw
                    .parse::<InferenceProviderKind>()
                    .map_err(|error| {
                        anyhow!(
                            "invalid sir_quality.deep_provider value '{}': {}",
                            provider_raw,
                            error
                        )
                    })
            })
            .transpose()?;
        owned_deep_pipeline = Some(
            SirPipeline::new(
                workspace.to_path_buf(),
                config.sir_quality.deep_concurrency.max(1),
                ProviderOverrides {
                    provider: deep_provider,
                    model: config.sir_quality.deep_model.clone(),
                    endpoint: config.sir_quality.deep_endpoint.clone(),
                    api_key_env: config.sir_quality.deep_api_key_env.clone(),
                },
            )
            .map(|pipeline| {
                pipeline.with_inference_timeout_secs(config.sir_quality.deep_timeout_secs)
            })
            .context("failed to initialize deep regeneration pipeline")?,
        );
    }
    let deep_pipeline = owned_deep_pipeline.as_ref().unwrap_or(&main_pipeline);
    let use_cot =
        args.deep && deep_pipeline.provider_name() == InferenceProviderKind::Qwen3Local.as_str();

    let total = candidates.len();
    let mut successes = 0usize;
    let mut failures = 0usize;
    let mut stdout = std::io::stdout();
    for candidate in candidates {
        let event = SymbolChangeEvent {
            file_path: candidate.symbol.file_path.clone(),
            language: candidate.symbol.language,
            added: Vec::new(),
            removed: Vec::new(),
            updated: vec![candidate.symbol.clone()],
        };
        let result = if args.deep {
            let enrichment = build_regeneration_enrichment_context(
                &store,
                &candidate,
                &priority_scores,
                config.sir_quality.deep_max_neighbors,
                config.sir_quality.deep_priority_threshold,
                config.sir_quality.deep_confidence_threshold,
            )?;
            let mut deep_specs = HashMap::new();
            deep_specs.insert(
                candidate.symbol.id.clone(),
                SirDeepPromptSpec {
                    enrichment,
                    use_cot,
                },
            );
            deep_pipeline.process_event_with_deep_specs(
                &store,
                &event,
                true,
                false,
                &mut stdout,
                Some(candidate.priority),
                SIR_GENERATION_PASS_REGENERATED,
                &deep_specs,
            )
        } else {
            main_pipeline.process_event_with_priority_and_pass(
                &store,
                &event,
                true,
                false,
                &mut stdout,
                Some(candidate.priority),
                SIR_GENERATION_PASS_REGENERATED,
            )
        };

        match result {
            Ok(stats) => {
                successes += stats.success_count;
                failures += stats.failure_count;
            }
            Err(err) => {
                failures += 1;
                tracing::warn!(
                    symbol_id = %candidate.symbol.id,
                    qualified_name = %candidate.symbol.qualified_name,
                    error = %err,
                    "regenerate symbol failed"
                );
            }
        }
    }

    println!(
        "Regenerated {} symbols. {} succeeded, {} failed.",
        total, successes, failures
    );
    Ok(())
}

fn build_regeneration_enrichment_context(
    store: &SqliteStore,
    candidate: &RegenerateCandidate,
    priority_scores: &HashMap<String, f64>,
    max_neighbors: usize,
    deep_priority_threshold: f64,
    deep_confidence_threshold: f64,
) -> Result<SirEnrichmentContext> {
    let file_rollup_id = synthetic_file_sir_id(
        candidate.symbol.language.as_str(),
        candidate.symbol.file_path.as_str(),
    );
    let file_intent = store
        .read_sir_blob(file_rollup_id.as_str())?
        .and_then(|blob| serde_json::from_str::<FileSir>(&blob).ok())
        .map(|sir| sir.intent.trim().to_owned())
        .unwrap_or_default();

    let mut neighbors = Vec::<(f64, String, String)>::new();
    for peer in store.list_symbols_for_file(candidate.symbol.file_path.as_str())? {
        if peer.id == candidate.symbol.id {
            continue;
        }
        let Some(blob) = store.read_sir_blob(peer.id.as_str())? else {
            continue;
        };
        let Ok(peer_sir) = serde_json::from_str::<SirAnnotation>(&blob) else {
            continue;
        };
        neighbors.push((
            priority_scores
                .get(peer.id.as_str())
                .copied()
                .unwrap_or(0.0),
            peer.qualified_name,
            peer_sir.intent,
        ));
    }
    neighbors.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
    });
    if max_neighbors > 0 && neighbors.len() > max_neighbors {
        neighbors.truncate(max_neighbors);
    }

    Ok(SirEnrichmentContext {
        file_intent: Some(file_intent),
        neighbor_intents: neighbors
            .into_iter()
            .map(|(_, name, intent)| (name, intent))
            .collect(),
        baseline_sir: Some(candidate.baseline_sir.clone()),
        priority_reason: format_regeneration_priority_reason(
            store,
            candidate.symbol.id.as_str(),
            candidate.priority,
            candidate.confidence as f64,
            deep_priority_threshold,
            deep_confidence_threshold,
        ),
    })
}

fn format_regeneration_priority_reason(
    store: &SqliteStore,
    symbol_id: &str,
    priority_score: f64,
    confidence: f64,
    deep_priority_threshold: f64,
    deep_confidence_threshold: f64,
) -> String {
    let mut reasons = Vec::<String>::new();
    if priority_score >= deep_priority_threshold {
        reasons.push(format!(
            "priority {:.2} at or above threshold {:.2}",
            priority_score, deep_priority_threshold
        ));
    }
    if confidence <= deep_confidence_threshold {
        reasons.push(format!(
            "confidence {:.2} at or below threshold {:.2}",
            confidence, deep_confidence_threshold
        ));
    }
    if let Ok(Some(metadata)) = store.get_symbol_metadata(symbol_id) {
        if metadata.is_public {
            reasons.push("public API symbol".to_owned());
        }
        let kind = metadata.kind.to_ascii_lowercase();
        if kind == "function" || kind == "method" {
            reasons.push("function/method".to_owned());
        }
    }
    if reasons.is_empty() {
        "selected for regeneration".to_owned()
    } else {
        reasons.join(" + ")
    }
}

fn truncate_display_name(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let keep = width.saturating_sub(3);
    let truncated = value.chars().take(keep).collect::<String>();
    format!("{truncated}...")
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

fn run_status_subcommand(workspace: &Path) -> Result<()> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let (total_symbols, symbols_with_sir) = store
        .count_symbols_with_sir()
        .context("failed to compute SIR coverage")?;
    let coverage_pct = if total_symbols > 0 {
        (symbols_with_sir as f64 / total_symbols as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "SIR Coverage: {} / {} ({coverage_pct:.1}%)",
        symbols_with_sir, total_symbols
    );
    Ok(())
}

fn run_remember_note_command(workspace: &Path, args: RememberArgs) -> Result<()> {
    run_remember_command(workspace, args).context("remember command failed")
}

fn run_recall_note_command(workspace: &Path, args: RecallArgs) -> Result<()> {
    run_recall_command(workspace, args).context("recall command failed")
}

fn run_notes_list_command(workspace: &Path, args: NotesArgs) -> Result<()> {
    run_notes_command(workspace, args).context("notes command failed")
}

fn run_ask_subcommand(workspace: &Path, args: AskArgs) -> Result<()> {
    run_ask_command(workspace, args).context("ask command failed")
}

fn run_mine_coupling_subcommand(workspace: &Path, args: MineCouplingArgs) -> Result<()> {
    run_mine_coupling_command(workspace, args).context("mine-coupling command failed")
}

fn run_blast_radius_subcommand(workspace: &Path, args: BlastRadiusArgs) -> Result<()> {
    run_blast_radius_command(workspace, args).context("blast-radius command failed")
}

fn run_coupling_report_subcommand(workspace: &Path, args: CouplingReportArgs) -> Result<()> {
    run_coupling_report_command(workspace, args).context("coupling-report command failed")
}

fn run_test_intents_subcommand(workspace: &Path, args: TestIntentsArgs) -> Result<()> {
    run_test_intents_command(workspace, args).context("test-intents command failed")
}

fn run_drift_report_subcommand(workspace: &Path, args: DriftReportArgs) -> Result<()> {
    run_drift_report_command(workspace, args).context("drift-report command failed")
}

fn run_drift_ack_subcommand(workspace: &Path, args: DriftAckArgs) -> Result<()> {
    run_drift_ack_command(workspace, args).context("drift-ack command failed")
}

fn run_communities_subcommand(workspace: &Path, args: CommunitiesArgs) -> Result<()> {
    run_communities_command(workspace, args).context("communities command failed")
}

fn run_trace_cause_subcommand(workspace: &Path, args: TraceCauseArgs) -> Result<()> {
    run_trace_cause_command(workspace, args).context("trace-cause command failed")
}

fn run_health_subcommand(workspace: &Path, args: HealthArgs) -> Result<()> {
    run_health_command(workspace, args).context("health command failed")
}

fn run_health_score_subcommand(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<()> {
    run_health_score_command(workspace, config, args).context("health-score command failed")
}

fn run_fsck_subcommand(workspace: &Path, args: FsckArgs) -> Result<()> {
    run_fsck(workspace, args.repair, args.verbose)
        .map(|_| ())
        .context("fsck command failed")
}

#[cfg(feature = "legacy-cozo")]
fn run_graph_migrate_subcommand(
    _workspace_from_global: &Path,
    args: aetherd::cli::GraphMigrateArgs,
) -> Result<()> {
    let workspace = args.workspace.canonicalize().with_context(|| {
        format!(
            "failed to resolve workspace path {}",
            args.workspace.display()
        )
    })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for graph migration")?;
    let result = runtime
        .block_on(migrate_cozo_to_surreal(&workspace, args.dry_run))
        .context("graph migration failed")?;
    eprintln!(
        "graph migration {}: symbols={}, edges={}",
        if result.dry_run {
            "dry-run"
        } else {
            "completed"
        },
        result.symbols_migrated,
        result.edges_migrated
    );
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

    match init_result {
        Ok(()) => Ok(()),
        Err(err)
            if err
                .to_string()
                .contains("global default trace dispatcher has already been set") =>
        {
            Ok(())
        }
        Err(err) => Err(anyhow!(
            "failed to initialize tracing subscriber (format={}): {err}",
            log_format.as_str()
        )),
    }
}

fn build_env_filter(configured_log_level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new(configured_log_level))
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_LEVEL))
}
use aetherd::sir_pipeline::{SIR_GENERATION_PASS_REGENERATED, SirDeepPromptSpec, SirPipeline};
