use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use aether_analysis::HealthAnalyzer;
use aether_config::{AetherConfig, GraphBackend};
use aether_core::{GitContext, SIR_STATUS_STALE, SymbolKind, normalize_path};
use aether_graph_algo::GraphAlgorithmEdge;
use aether_health::history::{
    create_table_if_needed, read_latest_report, read_report_by_commit_prefix, write_score,
};
use aether_health::{
    FileCommunityConfig, FileSymbol, PlannerDiagnostics, ScoreReport, SemanticFileInput,
    SemanticInput, SplitSuggestion, compare_reports, compute_workspace_score,
    compute_workspace_score_filtered, compute_workspace_score_with_signals,
    detect_file_communities, format_compare_json, format_compare_table, format_json, format_table,
    suggest_split,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{
    SqliteStore, Store, SurrealGraphStore, SymbolRecord, VectorStore, open_vector_store,
};
use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

#[derive(Debug)]
pub struct HealthScoreExecution {
    pub report: ScoreReport,
    pub rendered: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone)]
struct SplitSuggestionEntry {
    crate_name: String,
    crate_score: u32,
    status: String,
    message: Option<String>,
    suggestion: Option<SplitSuggestion>,
    diagnostics: Option<PlannerDiagnostics>,
}

pub fn run_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<()> {
    let execution = execute_health_score_command(workspace, config, args)?;
    let mut stdout = std::io::stdout();
    stdout
        .write_all(execution.rendered.as_bytes())
        .context("failed to write health-score output")?;
    if !execution.rendered.ends_with('\n') {
        writeln!(&mut stdout).context("failed to terminate health-score output")?;
    }

    if execution.exit_code != 0 {
        std::process::exit(execution.exit_code);
    }

    Ok(())
}

pub fn execute_health_score_command(
    workspace: &Path,
    config: &AetherConfig,
    args: HealthScoreArgs,
) -> Result<HealthScoreExecution> {
    if args.compare.is_some() && !args.crate_filter.is_empty() {
        bail!("--compare cannot be combined with --crate");
    }

    let compare_baseline = load_compare_baseline(workspace, args.compare.as_deref())?;
    let mut report = compute_current_report(workspace, config, &args)?;

    let history_allowed = !args.no_history && args.crate_filter.is_empty();
    if history_allowed {
        attach_previous_history(workspace, &mut report)?;
    }

    let split_suggestions = if args.suggest_splits && compare_baseline.is_none() {
        Some(collect_split_suggestion_entries(
            workspace,
            config,
            &report,
            args.semantic,
        ))
    } else {
        None
    };

    let mut rendered = if let Some(before) = compare_baseline.as_ref() {
        let compare = compare_reports(before, &report);
        match args.output {
            HealthScoreOutputFormat::Table => format_compare_table(&compare),
            HealthScoreOutputFormat::Json => format_compare_json(&compare),
        }
    } else {
        match args.output {
            HealthScoreOutputFormat::Table => format_table(&report),
            HealthScoreOutputFormat::Json => {
                if let Some(entries) = split_suggestions.as_deref() {
                    render_health_report_json(&report, entries)
                } else {
                    format_json(&report)
                }
            }
        }
    };

    if args.suggest_splits
        && matches!(args.output, HealthScoreOutputFormat::Table)
        && let Some(entries) = split_suggestions.as_deref()
        && let Some(section) = render_split_suggestions(entries)
    {
        rendered.push_str("\n\n");
        rendered.push_str(&section);
    }

    if history_allowed {
        write_current_history(workspace, &report)?;
    }

    let exit_code = if args
        .fail_below
        .is_some_and(|threshold| report.workspace_score < threshold)
    {
        1
    } else {
        0
    };

    Ok(HealthScoreExecution {
        report,
        rendered,
        exit_code,
    })
}

fn compute_current_report(
    workspace: &Path,
    config: &AetherConfig,
    args: &HealthScoreArgs,
) -> Result<ScoreReport> {
    let report = if args.semantic {
        let git = GitContext::open(workspace);
        let semantic = load_semantic_input(workspace)?;
        compute_workspace_score_with_signals(
            workspace,
            &config.health_score,
            &args.crate_filter,
            git.as_ref(),
            semantic.as_ref(),
        )
    } else if args.crate_filter.is_empty() {
        compute_workspace_score(workspace, &config.health_score)
    } else {
        compute_workspace_score_filtered(workspace, &config.health_score, &args.crate_filter)
    }
    .context("failed to compute health score")?;

    Ok(report)
}

fn load_compare_baseline(workspace: &Path, compare: Option<&str>) -> Result<Option<ScoreReport>> {
    let Some(compare) = compare.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(conn) = open_history_connection(workspace)? else {
        bail!("health score history is unavailable for --compare");
    };

    let baseline = if compare.eq_ignore_ascii_case("last") {
        read_latest_report(&conn).context("failed to read latest health score history")?
    } else {
        read_report_by_commit_prefix(&conn, compare).with_context(|| {
            format!("failed to read health score history for commit '{compare}'")
        })?
    };

    baseline
        .ok_or_else(|| {
            anyhow::anyhow!(if compare.eq_ignore_ascii_case("last") {
                "no historical health score is available".to_owned()
            } else {
                format!("no historical health score found for commit '{compare}'")
            })
        })
        .map(Some)
}

fn attach_previous_history(workspace: &Path, report: &mut ScoreReport) -> Result<()> {
    let Some(conn) = open_history_connection(workspace)? else {
        return Ok(());
    };
    if let Some(previous) =
        read_latest_report(&conn).context("failed to read previous health score")?
    {
        report.previous_score = Some(previous.workspace_score);
        report.delta = Some(report.workspace_score as i32 - previous.workspace_score as i32);
    }
    Ok(())
}

fn write_current_history(workspace: &Path, report: &ScoreReport) -> Result<()> {
    let Some(conn) = open_history_connection(workspace)? else {
        return Ok(());
    };
    write_score(&conn, report).context("failed to write health score history")?;
    Ok(())
}

fn open_history_connection(workspace: &Path) -> Result<Option<Connection>> {
    let sqlite_path = workspace.join(".aether").join("meta.sqlite");
    if !sqlite_path.exists() {
        return Ok(None);
    }

    let conn = Connection::open(&sqlite_path)
        .with_context(|| format!("failed to open {}", sqlite_path.display()))?;
    create_table_if_needed(&conn).context("failed to prepare health score history table")?;
    Ok(Some(conn))
}

fn render_health_report_json(report: &ScoreReport, entries: &[SplitSuggestionEntry]) -> String {
    let mut value = match serde_json::to_value(report) {
        Ok(value) => value,
        Err(err) => {
            return format!("{{\"error\":\"failed to serialize health report: {err}\"}}");
        }
    };

    let Some(object) = value.as_object_mut() else {
        return format_json(report);
    };
    object.insert(
        "split_suggestions".to_owned(),
        serde_json::Value::Array(
            entries
                .iter()
                .map(split_entry_to_json_value)
                .collect::<Vec<_>>(),
        ),
    );
    serde_json::to_string_pretty(&value)
        .unwrap_or_else(|err| format!("{{\"error\":\"failed to serialize health report: {err}\"}}"))
}

fn split_entry_to_json_value(entry: &SplitSuggestionEntry) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "crate_name".to_owned(),
        serde_json::Value::String(entry.crate_name.clone()),
    );
    object.insert(
        "crate_score".to_owned(),
        serde_json::Value::Number(entry.crate_score.into()),
    );
    object.insert(
        "status".to_owned(),
        serde_json::Value::String(entry.status.clone()),
    );
    if let Some(message) = entry.message.as_ref() {
        object.insert(
            "message".to_owned(),
            serde_json::Value::String(message.clone()),
        );
    }
    if let Some(suggestion) = entry.suggestion.as_ref()
        && let Ok(value) = serde_json::to_value(suggestion)
    {
        object.insert("suggestion".to_owned(), value);
    }
    if let Some(diagnostics) = entry.diagnostics.as_ref()
        && let Ok(value) = serde_json::to_value(diagnostics)
    {
        object.insert("diagnostics".to_owned(), value);
    }
    serde_json::Value::Object(object)
}

fn collect_split_suggestion_entries(
    workspace: &Path,
    config: &AetherConfig,
    report: &ScoreReport,
    semantic_enabled: bool,
) -> Vec<SplitSuggestionEntry> {
    let qualifying = report
        .crates
        .iter()
        .filter(|crate_score| crate_score.score <= 50)
        .collect::<Vec<_>>();
    if qualifying.is_empty() {
        return Vec::new();
    }
    if !semantic_enabled {
        return qualifying
            .into_iter()
            .map(|crate_score| {
                split_entry(
                    crate_score.name.clone(),
                    crate_score.score,
                    "skipped",
                    Some("split suggestions require --semantic".to_owned()),
                    None,
                    None,
                )
            })
            .collect();
    }
    if config.storage.graph_backend != GraphBackend::Surreal {
        return qualifying
            .into_iter()
            .map(|crate_score| {
                split_entry(
                    crate_score.name.clone(),
                    crate_score.score,
                    "unavailable",
                    Some(
                        "split suggestions require storage.graph_backend = \"surreal\"".to_owned(),
                    ),
                    None,
                    None,
                )
            })
            .collect();
    }

    let store = match SqliteStore::open_readonly(workspace) {
        Ok(store) => store,
        Err(_) => {
            return qualifying
                .into_iter()
                .map(|crate_score| {
                    split_entry(
                        crate_score.name.clone(),
                        crate_score.score,
                        "unavailable",
                        Some("workspace index is not readable".to_owned()),
                        None,
                        None,
                    )
                })
                .collect();
        }
    };

    let loaded =
        match load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
        {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return qualifying
                    .into_iter()
                    .map(|crate_score| {
                        split_entry(
                            crate_score.name.clone(),
                            crate_score.score,
                            "unavailable",
                            Some("embeddings are disabled for this workspace".to_owned()),
                            None,
                            None,
                        )
                    })
                    .collect();
            }
            Err(err) => {
                return qualifying
                    .into_iter()
                    .map(|crate_score| {
                        split_entry(
                            crate_score.name.clone(),
                            crate_score.score,
                            "unavailable",
                            Some(format!("failed to load embedding provider: {err}")),
                            None,
                            None,
                        )
                    })
                    .collect();
            }
        };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            return qualifying
                .into_iter()
                .map(|crate_score| {
                    split_entry(
                        crate_score.name.clone(),
                        crate_score.score,
                        "unavailable",
                        Some(format!(
                            "failed to build runtime for split suggestions: {err}"
                        )),
                        None,
                        None,
                    )
                })
                .collect();
        }
    };

    let graph = match runtime.block_on(SurrealGraphStore::open_readonly(workspace)) {
        Ok(graph) => graph,
        Err(err) => {
            return qualifying
                .into_iter()
                .map(|crate_score| {
                    split_entry(
                        crate_score.name.clone(),
                        crate_score.score,
                        "unavailable",
                        Some(format!("failed to open surreal graph: {err}")),
                        None,
                        None,
                    )
                })
                .collect();
        }
    };
    let all_edges = match runtime.block_on(graph.list_dependency_edges()) {
        Ok(edges) => edges
            .into_iter()
            .map(|edge| GraphAlgorithmEdge {
                source_id: edge.source_symbol_id,
                target_id: edge.target_symbol_id,
                edge_kind: edge.edge_kind,
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            return qualifying
                .into_iter()
                .map(|crate_score| {
                    split_entry(
                        crate_score.name.clone(),
                        crate_score.score,
                        "unavailable",
                        Some(format!("failed to load dependency edges: {err}")),
                        None,
                        None,
                    )
                })
                .collect();
        }
    };

    let vector_store = match runtime.block_on(open_vector_store(workspace)) {
        Ok(vector_store) => vector_store,
        Err(err) => {
            return qualifying
                .into_iter()
                .map(|crate_score| {
                    split_entry(
                        crate_score.name.clone(),
                        crate_score.score,
                        "unavailable",
                        Some(format!("failed to open vector store: {err}")),
                        None,
                        None,
                    )
                })
                .collect();
        }
    };
    let planner_config = FileCommunityConfig {
        semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
        semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
        community_resolution: config.planner.community_resolution,
        min_community_size: config.planner.min_community_size,
    };

    qualifying
        .into_iter()
        .map(|crate_score| {
            build_split_suggestion_entry(
                crate_score.name.as_str(),
                crate_score.score,
                crate_score.metrics.max_file_path.as_deref(),
                &store,
                all_edges.as_slice(),
                vector_store.as_ref(),
                loaded.provider_name.as_str(),
                loaded.model_name.as_str(),
                &planner_config,
                &runtime,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_split_suggestion_entry(
    crate_name: &str,
    crate_score: u32,
    file_path: Option<&str>,
    store: &SqliteStore,
    all_edges: &[GraphAlgorithmEdge],
    vector_store: &dyn VectorStore,
    provider_name: &str,
    model_name: &str,
    planner_config: &FileCommunityConfig,
    runtime: &tokio::runtime::Runtime,
) -> SplitSuggestionEntry {
    let Some(file_path) = file_path else {
        return split_entry(
            crate_name.to_owned(),
            crate_score,
            "no_split",
            Some("hotspot file path is unavailable".to_owned()),
            None,
            None,
        );
    };

    let symbol_records = match store.list_symbols_for_file(file_path) {
        Ok(symbols) => symbols,
        Err(err) => {
            return split_entry(
                crate_name.to_owned(),
                crate_score,
                "unavailable",
                Some(format!("failed to load symbols for {file_path}: {err}")),
                None,
                None,
            );
        }
    };
    if symbol_records.is_empty() {
        return split_entry(
            crate_name.to_owned(),
            crate_score,
            "unavailable",
            Some(format!("no indexed symbols were found for {file_path}")),
            None,
            None,
        );
    }

    let symbol_ids = symbol_records
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<Vec<_>>();
    let symbol_id_set = symbol_ids
        .iter()
        .map(|symbol_id| symbol_id.as_str())
        .collect::<HashSet<_>>();
    let structural_edges = all_edges
        .iter()
        .filter(|edge| {
            symbol_id_set.contains(edge.source_id.as_str())
                && symbol_id_set.contains(edge.target_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();

    let embedding_records = match runtime.block_on(vector_store.list_embeddings_for_symbols(
        provider_name,
        model_name,
        symbol_ids.as_slice(),
    )) {
        Ok(records) => records,
        Err(err) => {
            return split_entry(
                crate_name.to_owned(),
                crate_score,
                "unavailable",
                Some(format!("failed to load embeddings for {file_path}: {err}")),
                None,
                None,
            );
        }
    };
    let embedding_by_id = embedding_records
        .into_iter()
        .map(|record| (record.symbol_id, record.embedding))
        .collect::<HashMap<_, _>>();

    let file_symbols = symbol_records
        .iter()
        .map(|record| build_file_symbol(store, record, &embedding_by_id))
        .collect::<Vec<_>>();
    let (assignments, diagnostics) = detect_file_communities(
        structural_edges.as_slice(),
        file_symbols.as_slice(),
        planner_config,
    );
    if assignments.is_empty() {
        return split_entry(
            crate_name.to_owned(),
            crate_score,
            "no_split",
            Some("all non-test symbols were loners after rescue passes".to_owned()),
            None,
            Some(diagnostics),
        );
    }

    let community_count = assignments
        .iter()
        .map(|(_, community_id)| *community_id)
        .collect::<HashSet<_>>()
        .len();
    if community_count < 2 {
        return split_entry(
            crate_name.to_owned(),
            crate_score,
            "no_split",
            Some("only one actionable community was detected".to_owned()),
            None,
            Some(diagnostics),
        );
    }

    match suggest_split(
        file_path,
        crate_score,
        structural_edges.as_slice(),
        file_symbols.as_slice(),
        planner_config,
    ) {
        Some((suggestion, diagnostics)) => split_entry(
            crate_name.to_owned(),
            crate_score,
            "suggested",
            None,
            Some(suggestion),
            Some(diagnostics),
        ),
        None => split_entry(
            crate_name.to_owned(),
            crate_score,
            "no_split",
            Some("no actionable split suggestion was produced".to_owned()),
            None,
            Some(diagnostics),
        ),
    }
}

fn build_file_symbol(
    store: &SqliteStore,
    record: &SymbolRecord,
    embedding_by_id: &HashMap<String, Vec<f32>>,
) -> FileSymbol {
    FileSymbol {
        symbol_id: record.id.clone(),
        name: symbol_leaf_name(record.qualified_name.as_str()).to_owned(),
        qualified_name: record.qualified_name.clone(),
        kind: parse_symbol_kind(record.kind.as_str()),
        is_test: symbol_is_test(store, record),
        embedding: embedding_by_id.get(record.id.as_str()).cloned(),
    }
}

fn split_entry(
    crate_name: String,
    crate_score: u32,
    status: &str,
    message: Option<String>,
    suggestion: Option<SplitSuggestion>,
    diagnostics: Option<PlannerDiagnostics>,
) -> SplitSuggestionEntry {
    SplitSuggestionEntry {
        crate_name,
        crate_score,
        status: status.to_owned(),
        message,
        suggestion,
        diagnostics,
    }
}

fn render_split_suggestions(entries: &[SplitSuggestionEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }

    let mut lines = vec!["Split suggestions:".to_owned()];
    for entry in entries {
        lines.push(format!("{} - {}/100", entry.crate_name, entry.crate_score));
        lines.push(format!("  status: {}", entry.status));
        if let Some(message) = &entry.message {
            lines.push(format!("  message: {message}"));
        }
        if let Some(suggestion) = &entry.suggestion {
            lines.push(format!("  target_file: {}", suggestion.target_file));
            if let Some(diagnostics) = &entry.diagnostics {
                lines.push(format!(
                    "  confidence: {} ({:.2})",
                    diagnostics.confidence_label, diagnostics.confidence
                ));
                lines.push(format!("  stability: {:.2}", diagnostics.stability_score));
            }
            lines.push(format!(
                "  expected impact: {}",
                suggestion.expected_score_impact
            ));
            for module in &suggestion.suggested_modules {
                lines.push(format!(
                    "  - {} -> {}: {} ({})",
                    module.name,
                    module.suggested_file_path,
                    module.symbols.join(", "),
                    module.reason
                ));
            }
        }
        if let Some(diagnostics) = &entry.diagnostics {
            append_planner_diagnostics(&mut lines, diagnostics);
        }
        lines.push(String::new());
    }

    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }
    Some(lines.join("\n"))
}

fn append_planner_diagnostics(lines: &mut Vec<String>, diagnostics: &PlannerDiagnostics) {
    lines.push("  diagnostics:".to_owned());
    lines.push(format!("    symbols_total: {}", diagnostics.symbols_total));
    lines.push(format!(
        "    symbols_filtered_test: {}",
        diagnostics.symbols_filtered_test
    ));
    lines.push(format!(
        "    symbols_anchored_type: {}",
        diagnostics.symbols_anchored_type
    ));
    lines.push(format!(
        "    symbols_rescued_container: {}",
        diagnostics.symbols_rescued_container
    ));
    lines.push(format!(
        "    symbols_rescued_semantic: {}",
        diagnostics.symbols_rescued_semantic
    ));
    lines.push(format!("    symbols_loner: {}", diagnostics.symbols_loner));
    lines.push(format!(
        "    communities_before_merge: {}",
        diagnostics.communities_before_merge
    ));
    lines.push(format!(
        "    communities_after_merge: {}",
        diagnostics.communities_after_merge
    ));
    lines.push(format!(
        "    embedding_coverage_pct: {:.2}",
        diagnostics.embedding_coverage_pct
    ));
    lines.push(format!("    confidence: {:.2}", diagnostics.confidence));
    lines.push(format!(
        "    confidence_label: {}",
        diagnostics.confidence_label
    ));
    lines.push(format!(
        "    stability_score: {:.2}",
        diagnostics.stability_score
    ));
}

fn parse_symbol_kind(raw: &str) -> SymbolKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "variable" => SymbolKind::Variable,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "interface" => SymbolKind::Interface,
        "type_alias" => SymbolKind::TypeAlias,
        _ => SymbolKind::Function,
    }
}

fn symbol_leaf_name(qualified_name: &str) -> &str {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
}

fn symbol_is_test(store: &SqliteStore, record: &SymbolRecord) -> bool {
    if store
        .list_test_intents_for_symbol(record.id.as_str())
        .map(|records| !records.is_empty())
        .unwrap_or(false)
    {
        return true;
    }

    let leaf_name = symbol_leaf_name(record.qualified_name.as_str()).to_ascii_lowercase();
    if leaf_name.starts_with("test_") {
        return true;
    }

    let normalized_path = normalize_path(record.file_path.as_str()).to_ascii_lowercase();
    normalized_path.starts_with("tests/") || normalized_path.contains("/tests/")
}

fn load_semantic_input(workspace: &Path) -> Result<Option<SemanticInput>> {
    let sqlite_path = workspace.join(".aether").join("meta.sqlite");
    if !sqlite_path.exists() {
        return Ok(None);
    }

    let analyzer =
        HealthAnalyzer::new(workspace).context("failed to initialize health analyzer")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for semantic health scoring")?;
    let centrality = runtime
        .block_on(analyzer.centrality_by_file())
        .context("failed to collect centrality by file")?;
    if centrality.files.is_empty() && !centrality.notes.is_empty() {
        return Ok(None);
    }

    let store = match SqliteStore::open_readonly(workspace) {
        Ok(store) => store,
        Err(_) => return Ok(None),
    };

    let drift_by_symbol = latest_semantic_drift_by_symbol(&store)?;
    let community_by_symbol = store
        .list_latest_community_snapshot()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| (entry.symbol_id, entry.community_id))
        .collect::<HashMap<_, _>>();

    let mut files = HashMap::new();
    for entry in centrality.files {
        let path = normalize_path(entry.file.as_str());
        let symbols = store
            .list_symbols_for_file(path.as_str())
            .with_context(|| format!("failed to list symbols for {}", path))?;
        if symbols.is_empty() {
            continue;
        }

        let drifted_symbol_count = symbols
            .iter()
            .filter(|symbol| {
                drift_by_symbol
                    .get(symbol.id.as_str())
                    .is_some_and(|magnitude| *magnitude > 0.3)
            })
            .count();
        let stale_or_missing_sir_count = symbols
            .iter()
            .filter(|symbol| {
                store
                    .get_sir_meta(symbol.id.as_str())
                    .ok()
                    .flatten()
                    .is_none_or(|meta| {
                        meta.sir_status
                            .trim()
                            .eq_ignore_ascii_case(SIR_STATUS_STALE)
                    })
            })
            .count();
        let mut community_freq = HashMap::new();
        for symbol in &symbols {
            if let Some(community_id) = community_by_symbol.get(symbol.id.as_str()).copied() {
                *community_freq.entry(community_id).or_insert(0usize) += 1;
            }
        }
        let threshold = 3_usize.max((symbols.len() as f64 * 0.2).ceil() as usize);
        let community_count = community_freq
            .values()
            .filter(|&&count| count >= threshold)
            .count();
        let has_test_coverage = symbols.iter().any(|symbol| {
            store
                .list_test_intents_for_symbol(symbol.id.as_str())
                .map(|records| !records.is_empty())
                .unwrap_or(false)
        });

        files.insert(
            path,
            SemanticFileInput {
                max_pagerank: entry.max_pagerank,
                symbol_count: symbols.len(),
                drifted_symbol_count,
                stale_or_missing_sir_count,
                community_count,
                has_test_coverage,
            },
        );
    }

    Ok(Some(SemanticInput {
        workspace_max_pagerank: centrality.workspace_max_pagerank,
        files,
    }))
}

fn latest_semantic_drift_by_symbol(store: &SqliteStore) -> Result<HashMap<String, f64>> {
    let mut drift_by_symbol = HashMap::new();
    for record in store
        .list_drift_results(true)
        .context("failed to list semantic drift results")?
    {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };

        drift_by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(drift_by_symbol)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_config::AetherConfig;
    use aether_core::EdgeKind;
    use aether_graph_algo::GraphAlgorithmEdge;
    use aether_store::{
        CommunitySnapshotRecord, DriftResultRecord, GraphStore, ResolvedEdge, SqliteStore, Store,
        SurrealGraphStore, SymbolEmbeddingRecord, SymbolRecord, TestIntentRecord,
        open_vector_store,
    };
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{
        build_split_suggestion_entry, execute_health_score_command, render_health_report_json,
        render_split_suggestions,
    };
    use crate::cli::{HealthScoreArgs, HealthScoreOutputFormat};

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("test file path parent")).expect("create parent");
        fs::write(path, content).expect("write file");
    }

    fn create_workspace() -> tempfile::TempDir {
        let temp = tempdir().expect("tempdir");
        write_file(
            &temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/example\"]\nresolver = \"2\"\n",
        );
        write_file(
            &temp.path().join("crates/example/Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        temp
    }

    fn now_millis() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn write_semantic_config(workspace: &Path) {
        write_file(
            &workspace.join(".aether/config.toml"),
            r#"[storage]
mirror_sir_files = true
graph_backend = "surreal"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
model = "qwen3-embeddings-4B"
"#,
        );
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_repo(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["config", "user.name", "Aether Test"]);
        run_git(
            workspace,
            &["config", "user.email", "aether-test@example.com"],
        );
    }

    fn commit_all(workspace: &Path, message: &str) {
        run_git(workspace, &["add", "."]);
        run_git(workspace, &["commit", "-m", message]);
    }

    fn git_head_short(workspace: &Path) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(workspace)
            .output()
            .expect("git rev-parse");
        assert!(
            output.status.success(),
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("utf8 git output")
            .trim()
            .to_owned()
    }

    fn hot_health_config() -> AetherConfig {
        let mut config = AetherConfig::default();
        config.health_score.file_loc_warn = 1;
        config.health_score.file_loc_fail = 2;
        config.health_score.trait_method_warn = 1;
        config.health_score.trait_method_fail = 2;
        config
    }

    fn symbol(id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: now_millis(),
        }
    }

    fn embedding_record(symbol_id: &str, embedding: Vec<f32>) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: format!("sir-{symbol_id}"),
            provider: "qwen3_local".to_owned(),
            model: "qwen3-embeddings-4B".to_owned(),
            embedding,
            updated_at: now_millis(),
        }
    }

    fn planner_edge(source_id: &str, target_id: &str) -> GraphAlgorithmEdge {
        GraphAlgorithmEdge {
            source_id: source_id.to_owned(),
            target_id: target_id.to_owned(),
            edge_kind: "calls".to_owned(),
        }
    }

    fn seed_surreal_graph_snapshot(
        workspace: &Path,
        symbols: &[SymbolRecord],
        edges: &[(&str, &str)],
    ) {
        let seed_workspace = tempdir().expect("seed workspace");
        write_file(&seed_workspace.path().join("Cargo.toml"), "[workspace]\n");
        fs::create_dir_all(seed_workspace.path().join(".aether")).expect("create seed .aether");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let graph = SurrealGraphStore::open(seed_workspace.path())
                .await
                .expect("open surreal graph");
            for symbol in symbols {
                graph
                    .upsert_symbol_node(symbol)
                    .await
                    .expect("upsert symbol node");
            }
            for (source, target) in edges {
                graph
                    .upsert_edge(&ResolvedEdge {
                        source_id: (*source).to_owned(),
                        target_id: (*target).to_owned(),
                        edge_kind: EdgeKind::Calls,
                        file_path: "crates/example/src/lib.rs".to_owned(),
                    })
                    .await
                    .expect("upsert edge");
            }
        });
        drop(runtime);

        let source_graph = seed_workspace.path().join(".aether/graph");
        let target_graph = workspace.join(".aether/graph");
        fs::create_dir_all(workspace.join(".aether")).expect("create workspace .aether");
        if target_graph.exists() {
            fs::remove_dir_all(&target_graph).expect("remove existing graph dir");
        }
        fs::rename(&source_graph, &target_graph).expect("move surreal graph dir");
        let lock_file = target_graph.join("LOCK");
        if lock_file.exists() {
            fs::remove_file(lock_file).expect("remove stale surreal lock");
        }
    }

    fn default_args() -> HealthScoreArgs {
        HealthScoreArgs {
            output: HealthScoreOutputFormat::Json,
            fail_below: None,
            no_history: false,
            crate_filter: Vec::new(),
            semantic: false,
            suggest_splits: false,
            compare: None,
        }
    }

    #[test]
    fn fail_below_exit_code() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );

        let mut args = default_args();
        args.fail_below = Some(100);
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("health-score execution");

        assert_eq!(execution.exit_code, 1);
    }

    #[test]
    fn no_aether_dir_no_error() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() {}\n",
        );

        let execution = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("health-score execution");

        assert_eq!(execution.exit_code, 0);
        assert!(!workspace.path().join(".aether").exists());
    }

    #[test]
    fn history_written_and_delta() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("create sqlite");

        let first = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("first run");
        assert!(first.report.previous_score.is_none());

        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        let second = execute_health_score_command(
            workspace.path(),
            &AetherConfig::default(),
            default_args(),
        )
        .expect("second run");

        assert!(second.report.previous_score.is_some());
        assert!(second.report.delta.is_some());
    }

    #[test]
    fn filtered_run_skips_history() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() { let _ = \"cozo\"; let _ = \"cozo\"; }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        let conn = Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("sqlite");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS health_score_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_at INTEGER NOT NULL,
                git_commit TEXT,
                score INTEGER NOT NULL,
                score_json TEXT NOT NULL,
                UNIQUE(git_commit)
            )",
            [],
        )
        .expect("create table");

        let mut args = default_args();
        args.crate_filter = vec!["example".to_owned()];
        execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
            .expect("filtered run");

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM health_score_history", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("row count");
        assert_eq!(row_count, 0);
    }

    #[test]
    fn semantic_flag_without_index_gracefully_falls_back() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() {}\n",
        );

        let mut args = default_args();
        args.semantic = true;
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("semantic fallback execution");

        assert_eq!(execution.report.schema_version, 2);
        assert!(
            execution
                .report
                .crates
                .iter()
                .all(|crate_score| !crate_score.signal_availability.semantic_available)
        );
    }

    #[test]
    fn semantic_mode_populates_git_and_semantic_signals() {
        let workspace = create_workspace();
        write_semantic_config(workspace.path());
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() -> i32 { 1 }\npub fn beta() -> i32 { alpha() }\n",
        );
        init_git_repo(workspace.path());
        commit_all(workspace.path(), "initial");
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub fn alpha() -> i32 { 2 }\npub fn beta() -> i32 { alpha() + 1 }\n",
        );
        commit_all(workspace.path(), "update");

        let store = SqliteStore::open(workspace.path()).expect("open sqlite");
        let symbols = vec![
            symbol("sym-a", "crate::alpha", "crates/example/src/lib.rs"),
            symbol("sym-b", "crate::beta", "crates/example/src/lib.rs"),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }
        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: "drift-a".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "crates/example/src/lib.rs".to_owned(),
                symbol_name: "crate::alpha".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.8),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some("a".to_owned()),
                commit_range_end: Some("b".to_owned()),
                drift_summary: Some("alpha changed".to_owned()),
                detail_json: "{}".to_owned(),
                detected_at: now_millis(),
                is_acknowledged: false,
            }])
            .expect("seed drift");
        store
            .replace_community_snapshot(
                "snapshot-1",
                now_millis(),
                &[
                    CommunitySnapshotRecord {
                        snapshot_id: "snapshot-1".to_owned(),
                        symbol_id: "sym-a".to_owned(),
                        community_id: 1,
                        captured_at: now_millis(),
                    },
                    CommunitySnapshotRecord {
                        snapshot_id: "snapshot-1".to_owned(),
                        symbol_id: "sym-b".to_owned(),
                        community_id: 2,
                        captured_at: now_millis(),
                    },
                ],
            )
            .expect("seed communities");
        store
            .replace_test_intents_for_file(
                "tests/example_test.rs",
                &[TestIntentRecord {
                    intent_id: "intent-alpha".to_owned(),
                    file_path: "tests/example_test.rs".to_owned(),
                    test_name: "test_alpha".to_owned(),
                    intent_text: "covers alpha".to_owned(),
                    group_label: None,
                    language: "rust".to_owned(),
                    symbol_id: Some("sym-a".to_owned()),
                    created_at: now_millis(),
                    updated_at: now_millis(),
                }],
            )
            .expect("seed test intents");

        seed_surreal_graph_snapshot(workspace.path(), &symbols, &[("sym-a", "sym-b")]);

        let mut args = default_args();
        args.semantic = true;
        let execution =
            execute_health_score_command(workspace.path(), &AetherConfig::default(), args)
                .expect("semantic execution");

        assert_eq!(execution.report.schema_version, 2);
        let crate_score = &execution.report.crates[0];
        assert!(crate_score.signal_availability.git_available);
        assert!(crate_score.signal_availability.semantic_available);
        assert!(crate_score.git_signals.is_some());
        assert!(crate_score.semantic_signals.is_some());
        assert!(crate_score.score_breakdown.is_some());
        assert!(execution.rendered.contains("\"git_signals\""));
    }

    #[test]
    fn compare_with_last() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store { fn alpha(&self); }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("create sqlite");

        let mut first_args = default_args();
        first_args.output = HealthScoreOutputFormat::Table;
        execute_health_score_command(workspace.path(), &hot_health_config(), first_args)
            .expect("first run");

        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n",
        );

        let mut compare_args = default_args();
        compare_args.output = HealthScoreOutputFormat::Table;
        compare_args.compare = Some("last".to_owned());
        let execution =
            execute_health_score_command(workspace.path(), &hot_health_config(), compare_args)
                .expect("compare run");

        assert!(
            execution
                .rendered
                .contains("AETHER Health Score - Before/After Comparison")
        );
        assert!(execution.rendered.contains("Before:"));
        assert!(execution.rendered.contains("After:"));
        assert!(execution.rendered.contains("example"));
        assert!(execution.rendered.contains("Regressions:"));
    }

    #[test]
    fn compare_with_commit_hash() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store { fn alpha(&self); }\n",
        );
        fs::create_dir_all(workspace.path().join(".aether")).expect("create .aether");
        Connection::open(workspace.path().join(".aether/meta.sqlite")).expect("create sqlite");
        init_git_repo(workspace.path());
        commit_all(workspace.path(), "initial");

        let initial_commit = git_head_short(workspace.path());
        execute_health_score_command(workspace.path(), &hot_health_config(), default_args())
            .expect("initial score");

        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n",
        );
        commit_all(workspace.path(), "expand");

        let mut compare_args = default_args();
        compare_args.output = HealthScoreOutputFormat::Table;
        compare_args.compare = Some(initial_commit.clone());
        let execution =
            execute_health_score_command(workspace.path(), &hot_health_config(), compare_args)
                .expect("compare by commit");

        assert!(execution.rendered.contains(&initial_commit));
        assert!(execution.rendered.contains("example"));
    }

    #[test]
    fn suggest_splits_skips_without_semantic_data() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n",
        );

        let mut args = default_args();
        args.output = HealthScoreOutputFormat::Table;
        args.suggest_splits = true;
        let execution = execute_health_score_command(workspace.path(), &hot_health_config(), args)
            .expect("health-score execution");

        assert!(execution.rendered.contains("status: skipped"));
    }

    #[test]
    fn suggest_splits_appends_recommendation_for_hot_crate() {
        let workspace = create_workspace();
        write_semantic_config(workspace.path());
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n\npub fn sir_alpha() -> i32 { 1 }\npub fn sir_beta() -> i32 { sir_alpha() }\npub fn sir_gamma() -> i32 { sir_beta() }\npub fn sir_delta() -> i32 { sir_gamma() }\npub fn note_alpha() -> i32 { 3 }\npub fn note_beta() -> i32 { note_alpha() }\npub fn note_gamma() -> i32 { note_beta() }\npub fn note_delta() -> i32 { note_gamma() }\n",
        );

        let store = SqliteStore::open(workspace.path()).expect("open sqlite");
        let symbols = vec![
            symbol("sym-sir-a", "crate::sir_alpha", "crates/example/src/lib.rs"),
            symbol("sym-sir-b", "crate::sir_beta", "crates/example/src/lib.rs"),
            symbol("sym-sir-c", "crate::sir_gamma", "crates/example/src/lib.rs"),
            symbol("sym-sir-d", "crate::sir_delta", "crates/example/src/lib.rs"),
            symbol(
                "sym-note-a",
                "crate::note_alpha",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-b",
                "crate::note_beta",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-c",
                "crate::note_gamma",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-d",
                "crate::note_delta",
                "crates/example/src/lib.rs",
            ),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-a", vec![1.0, 0.0]))
            .expect("seed sir-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-b", vec![0.95, 0.05]))
            .expect("seed sir-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-c", vec![0.92, 0.08]))
            .expect("seed sir-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-d", vec![0.9, 0.1]))
            .expect("seed sir-d embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-a", vec![0.0, 1.0]))
            .expect("seed note-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-b", vec![0.05, 0.95]))
            .expect("seed note-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-c", vec![0.08, 0.92]))
            .expect("seed note-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-d", vec![0.1, 0.9]))
            .expect("seed note-d embedding");

        let all_edges = vec![
            planner_edge("sym-sir-a", "sym-sir-b"),
            planner_edge("sym-sir-b", "sym-sir-c"),
            planner_edge("sym-sir-c", "sym-sir-d"),
            planner_edge("sym-note-a", "sym-note-b"),
            planner_edge("sym-note-b", "sym-note-c"),
            planner_edge("sym-note-c", "sym-note-d"),
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace.path()))
            .expect("open vector store");
        let config = hot_health_config();
        let planner_config = super::FileCommunityConfig {
            semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
            semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
            community_resolution: config.planner.community_resolution,
            min_community_size: config.planner.min_community_size,
        };

        let entry = build_split_suggestion_entry(
            "example",
            37,
            Some("crates/example/src/lib.rs"),
            &store,
            all_edges.as_slice(),
            vector_store.as_ref(),
            "qwen3_local",
            "qwen3-embeddings-4B",
            &planner_config,
            &runtime,
        );
        let rendered =
            render_split_suggestions(std::slice::from_ref(&entry)).expect("render split section");

        assert_eq!(entry.status, "suggested");
        assert!(rendered.contains("Split suggestions:"));
        assert!(rendered.contains("sir_ops"));
        assert!(rendered.contains("note_ops"));
        assert!(rendered.contains("diagnostics:"));
        assert!(rendered.contains("stability:"));
    }

    #[test]
    fn suggest_splits_reports_unavailable_without_surreal_backend() {
        let workspace = create_workspace();
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n",
        );

        let mut config = hot_health_config();
        config.storage.graph_backend = aether_config::GraphBackend::Sqlite;

        let mut args = default_args();
        args.output = HealthScoreOutputFormat::Table;
        args.semantic = true;
        args.suggest_splits = true;
        let execution = execute_health_score_command(workspace.path(), &config, args)
            .expect("health-score execution");

        assert!(execution.rendered.contains("status: unavailable"));
        assert!(
            execution
                .rendered
                .contains("storage.graph_backend = \"surreal\"")
        );
    }

    #[test]
    fn suggest_splits_json_includes_sidecar_diagnostics() {
        let workspace = create_workspace();
        write_semantic_config(workspace.path());
        write_file(
            &workspace.path().join("crates/example/src/lib.rs"),
            "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n\npub fn sir_alpha() -> i32 { 1 }\npub fn sir_beta() -> i32 { sir_alpha() }\npub fn sir_gamma() -> i32 { sir_beta() }\npub fn sir_delta() -> i32 { sir_gamma() }\npub fn note_alpha() -> i32 { 3 }\npub fn note_beta() -> i32 { note_alpha() }\npub fn note_gamma() -> i32 { note_beta() }\npub fn note_delta() -> i32 { note_gamma() }\n",
        );

        let store = SqliteStore::open(workspace.path()).expect("open sqlite");
        let symbols = vec![
            symbol("sym-sir-a", "crate::sir_alpha", "crates/example/src/lib.rs"),
            symbol("sym-sir-b", "crate::sir_beta", "crates/example/src/lib.rs"),
            symbol("sym-sir-c", "crate::sir_gamma", "crates/example/src/lib.rs"),
            symbol("sym-sir-d", "crate::sir_delta", "crates/example/src/lib.rs"),
            symbol(
                "sym-note-a",
                "crate::note_alpha",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-b",
                "crate::note_beta",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-c",
                "crate::note_gamma",
                "crates/example/src/lib.rs",
            ),
            symbol(
                "sym-note-d",
                "crate::note_delta",
                "crates/example/src/lib.rs",
            ),
        ];
        for symbol in &symbols {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
        }
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-a", vec![1.0, 0.0]))
            .expect("seed sir-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-b", vec![0.95, 0.05]))
            .expect("seed sir-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-c", vec![0.92, 0.08]))
            .expect("seed sir-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-sir-d", vec![0.9, 0.1]))
            .expect("seed sir-d embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-a", vec![0.0, 1.0]))
            .expect("seed note-a embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-b", vec![0.05, 0.95]))
            .expect("seed note-b embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-c", vec![0.08, 0.92]))
            .expect("seed note-c embedding");
        store
            .upsert_symbol_embedding(embedding_record("sym-note-d", vec![0.1, 0.9]))
            .expect("seed note-d embedding");

        let all_edges = vec![
            planner_edge("sym-sir-a", "sym-sir-b"),
            planner_edge("sym-sir-b", "sym-sir-c"),
            planner_edge("sym-sir-c", "sym-sir-d"),
            planner_edge("sym-note-a", "sym-note-b"),
            planner_edge("sym-note-b", "sym-note-c"),
            planner_edge("sym-note-c", "sym-note-d"),
        ];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace.path()))
            .expect("open vector store");
        let config = hot_health_config();
        let planner_config = super::FileCommunityConfig {
            semantic_rescue_threshold: config.planner.semantic_rescue_threshold,
            semantic_rescue_max_k: config.planner.semantic_rescue_max_k,
            community_resolution: config.planner.community_resolution,
            min_community_size: config.planner.min_community_size,
        };
        let entry = build_split_suggestion_entry(
            "example",
            37,
            Some("crates/example/src/lib.rs"),
            &store,
            all_edges.as_slice(),
            vector_store.as_ref(),
            "qwen3_local",
            "qwen3-embeddings-4B",
            &planner_config,
            &runtime,
        );

        let mut args = default_args();
        args.output = HealthScoreOutputFormat::Json;
        args.semantic = false;
        args.suggest_splits = false;
        let execution = execute_health_score_command(workspace.path(), &hot_health_config(), args)
            .expect("health-score execution");
        let rendered = render_health_report_json(&execution.report, &[entry]);

        assert!(rendered.contains("\"split_suggestions\""));
        assert!(rendered.contains("\"diagnostics\""));
        assert!(rendered.contains("\"confidence_label\""));
        assert!(rendered.contains("\"stability_score\""));
    }
}
