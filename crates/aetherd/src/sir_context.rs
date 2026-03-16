use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use aether_core::{GitContext, Language, normalize_path};
use aether_health::{ScoreReport, workspace_health_config_or_default};
use aether_sir::{FileSir, SirAnnotation, canonicalize_file_sir_json, synthetic_file_sir_id};
use aether_store::{
    CouplingEdgeRecord, DriftStore, ProjectNoteStore, SirStateStore, SqliteStore,
    SurrealGraphStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore,
    TaskContextHistoryRecord, TestIntentStore, block_on_store_future,
    open_surreal_graph_store_readonly,
};
use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::cli::{ContextArgs, SirContextArgs};
use crate::context_presets::resolve_context_options;
use crate::context_renderers;
use crate::context_slicer::{SliceNeighbor, render_file_slice, slice_file_for_context};
use crate::sir_agent_support::{
    current_unix_timestamp_secs, first_line, first_sentence, format_relative_age,
    load_fresh_symbol_source, output_path, read_selector_file, resolve_symbol,
};
use crate::task_context::resolve_task_symbols_with_context;

const CHARS_PER_TOKEN: f64 = 3.5;
const MEMORY_LIMIT: usize = 10;
const GRAPH_LIMIT: usize = 12;
const DRIFT_LIMIT: usize = 10;

const LAYER_SUGGESTIONS: [(&str, usize); 9] = [
    ("source", 30),
    ("sir", 15),
    ("graph", 15),
    ("tests", 10),
    ("coupling", 8),
    ("memory", 7),
    ("health", 5),
    ("drift", 5),
    ("broader_graph", 5),
];

#[derive(Debug, Clone)]
pub enum ContextTarget {
    File {
        path: String,
    },
    Symbol {
        selector: String,
        file_hint: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFormat {
    Markdown,
    Json,
    Xml,
    Compact,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LayerSelection {
    pub sir: bool,
    pub source: bool,
    pub graph: bool,
    pub coupling: bool,
    pub health: bool,
    pub drift: bool,
    pub memory: bool,
    pub tests: bool,
}

impl LayerSelection {
    fn all() -> Self {
        Self {
            sir: true,
            source: true,
            graph: true,
            coupling: true,
            health: true,
            drift: true,
            memory: true,
            tests: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ExportDocument {
    pub generated_at: i64,
    pub project_overview: ProjectOverview,
    pub target_sections: Vec<TargetSection>,
    pub budget_usage: BudgetUsage,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProjectOverview {
    pub workspace: String,
    pub total_symbols: usize,
    pub symbols_with_sir: usize,
    pub sir_coverage_percent: f64,
    pub health: Option<WorkspaceHealthSummary>,
    pub drift: Option<WorkspaceDriftSummary>,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceHealthSummary {
    pub workspace_score: u32,
    pub severity: String,
    pub worst_crate: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceDriftSummary {
    pub active_findings: usize,
    pub semantic_findings: usize,
    pub max_magnitude: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetSection {
    pub target_kind: String,
    pub target_label: String,
    pub selector: Option<String>,
    pub file_path: Option<String>,
    pub language: Option<String>,
    pub file_sir: Option<FileSirContext>,
    pub symbols: Vec<ExportSymbolContext>,
    pub source: Option<SourceBlock>,
    pub immediate_graph: Vec<NeighborSummary>,
    pub broader_graph: Vec<NeighborSummary>,
    pub tests: Vec<TestGuard>,
    pub coupling: Vec<CouplingContext>,
    pub memory: Vec<MemoryContext>,
    pub health: Option<ExportHealthContext>,
    pub drift: Vec<DriftContext>,
    pub notices: Vec<String>,
}

impl TargetSection {
    fn file(path: String, language: Option<String>) -> Self {
        Self {
            target_kind: "file".to_owned(),
            target_label: path.clone(),
            selector: None,
            file_path: Some(path),
            language,
            file_sir: None,
            symbols: Vec::new(),
            source: None,
            immediate_graph: Vec::new(),
            broader_graph: Vec::new(),
            tests: Vec::new(),
            coupling: Vec::new(),
            memory: Vec::new(),
            health: None,
            drift: Vec::new(),
            notices: Vec::new(),
        }
    }

    fn symbol(selector: String, file_path: String, language: String) -> Self {
        Self {
            target_kind: "symbol".to_owned(),
            target_label: selector.clone(),
            selector: Some(selector),
            file_path: Some(file_path),
            language: Some(language),
            file_sir: None,
            symbols: Vec::new(),
            source: None,
            immediate_graph: Vec::new(),
            broader_graph: Vec::new(),
            tests: Vec::new(),
            coupling: Vec::new(),
            memory: Vec::new(),
            health: None,
            drift: Vec::new(),
            notices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceBlock {
    pub language: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileSirContext {
    pub intent: String,
    pub exports: Vec<String>,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub symbol_count: usize,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportSymbolContext {
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub language: String,
    pub staleness_score: Option<f64>,
    pub intent: String,
    pub behavior: Vec<String>,
    pub sir_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestGuard {
    pub test_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NeighborSummary {
    pub relationship: String,
    pub qualified_name: String,
    pub file_path: String,
    pub intent_summary: String,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct CouplingContext {
    pub file_path: String,
    pub fused_score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryContext {
    pub first_line: String,
    pub source_type: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportHealthContext {
    pub summary: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DriftContext {
    pub symbol_name: String,
    pub drift_type: String,
    pub drift_magnitude: Option<f32>,
    pub summary: String,
    pub detected_at: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct BudgetUsage {
    pub max_tokens: usize,
    pub used_tokens: usize,
    pub layers: Vec<LayerBudgetLine>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerBudgetLine {
    pub layer: String,
    pub suggested_tokens: usize,
    pub used_tokens: usize,
    pub status: BudgetStatus,
    pub included_items: usize,
    pub omitted_items: usize,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Included,
    Truncated,
    Omitted,
}

#[derive(Debug, Clone)]
struct PreparedItem<T> {
    value: T,
    cost_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedTargetSection {
    output: TargetSection,
    source: Option<PreparedItem<SourceBlock>>,
    file_sir: Option<PreparedItem<FileSirContext>>,
    symbols: Vec<PreparedItem<ExportSymbolContext>>,
    immediate_graph: Vec<PreparedItem<NeighborSummary>>,
    broader_graph: Vec<PreparedItem<NeighborSummary>>,
    tests: Vec<PreparedItem<TestGuard>>,
    coupling: Vec<PreparedItem<CouplingContext>>,
    memory: Vec<PreparedItem<MemoryContext>>,
    health: Option<PreparedItem<ExportHealthContext>>,
    drift: Vec<PreparedItem<DriftContext>>,
}

#[derive(Debug, Default)]
pub(crate) struct BudgetAllocator {
    max_tokens: usize,
    used_tokens: usize,
}

impl BudgetAllocator {
    fn new(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
        }
    }

    fn remaining(&self) -> usize {
        self.max_tokens.saturating_sub(self.used_tokens)
    }

    fn try_add(&mut self, content: &str) -> bool {
        let tokens = estimate_tokens(content);
        if tokens <= self.remaining() {
            self.used_tokens = self.used_tokens.saturating_add(tokens);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
struct LayerBudgetStats {
    used_tokens: usize,
    attempted_items: usize,
    included_items: usize,
}

impl LayerBudgetStats {
    fn note_attempt(&mut self, cost_text: &str, included: bool) {
        self.attempted_items = self.attempted_items.saturating_add(1);
        if included {
            self.included_items = self.included_items.saturating_add(1);
            self.used_tokens = self.used_tokens.saturating_add(estimate_tokens(cost_text));
        }
    }

    fn finish(self, layer: &str, max_tokens: usize, suggested_pct: usize) -> LayerBudgetLine {
        let omitted_items = self.attempted_items.saturating_sub(self.included_items);
        let status = if self.attempted_items == 0 || self.included_items == 0 {
            BudgetStatus::Omitted
        } else if omitted_items > 0 {
            BudgetStatus::Truncated
        } else {
            BudgetStatus::Included
        };

        LayerBudgetLine {
            layer: layer.to_owned(),
            suggested_tokens: max_tokens.saturating_mul(suggested_pct) / 100,
            used_tokens: self.used_tokens,
            status,
            included_items: self.included_items,
            omitted_items,
        }
    }
}

pub fn run_context_command(workspace: &Path, args: ContextArgs) -> Result<()> {
    let resolved = resolve_context_options(workspace, &args)?;
    let format = parse_context_format(resolved.format.as_str())?;
    let layers = parse_layer_selection(resolved.include.as_deref(), resolved.exclude.as_deref())?;
    let task_bias = resolved
        .task
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            args.branch
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    let store = SqliteStore::open(workspace).context("failed to open local store")?;

    let mut notices = Vec::new();
    let mut task_history_payload = None;
    let targets = if let Some(branch_name) = args
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let task_description = resolved
            .task
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(branch_name);
        let resolution = resolve_task_symbols_with_context(
            workspace,
            &store,
            task_description,
            Some(branch_name),
            20,
            0.6,
        )?;
        notices.extend(resolution.notices.iter().cloned());

        let ranked_symbol_ids = resolution
            .ranked_symbols
            .iter()
            .take(20)
            .map(|(symbol_id, _)| symbol_id.clone())
            .collect::<Vec<_>>();
        let ranked_rows = store
            .get_symbol_search_results_batch(ranked_symbol_ids.as_slice())
            .context("failed to resolve task-ranked symbols")?;
        let mut selected_symbol_ids = Vec::new();
        let mut selected_file_paths = Vec::new();
        let mut seen_files = HashSet::new();
        let mut targets = Vec::new();
        for symbol_id in ranked_symbol_ids {
            let Some(row) = ranked_rows.get(symbol_id.as_str()) else {
                notices.push(format!(
                    "task-ranked symbol metadata missing for {symbol_id}; skipping target"
                ));
                continue;
            };
            selected_symbol_ids.push(symbol_id.clone());
            if seen_files.insert(row.file_path.clone()) {
                selected_file_paths.push(row.file_path.clone());
            }
            targets.push(ContextTarget::Symbol {
                selector: row.qualified_name.clone(),
                file_hint: Some(row.file_path.clone()),
            });
        }
        if targets.is_empty() {
            notices.push("task context resolved no symbol targets".to_owned());
        }
        task_history_payload = Some((
            task_description.to_owned(),
            branch_name.to_owned(),
            selected_symbol_ids,
            selected_file_paths,
        ));
        targets
    } else {
        context_targets(workspace, &args)?
    };

    let health_report = match compute_workspace_health_report(workspace) {
        Ok(report) => report,
        Err(err) => {
            notices.push(format!("health score unavailable — {err}"));
            None
        }
    };
    let overview = build_project_overview(workspace, &store, health_report.as_ref(), &mut notices)?;

    let mut prepared = Vec::new();
    for target in &targets {
        prepared.push(prepare_target_section(
            workspace,
            &store,
            target,
            layers,
            resolved.depth,
            task_bias,
            health_report.as_ref(),
            resolved.context_lines,
        )?);
    }

    let document = allocate_export_document(overview, prepared, resolved.budget, notices);
    let rendered = render_export_document(&document, format);

    if let Some(path) = args.output.as_deref() {
        let path = output_path(path);
        fs::write(&path, rendered)
            .with_context(|| format!("failed to write output file {}", path.display()))?;
    } else {
        let mut out = std::io::stdout();
        out.write_all(rendered.as_bytes())
            .context("failed to write context output")?;
        if !rendered.ends_with('\n') {
            writeln!(&mut out).context("failed to write trailing newline")?;
        }
    }

    if let Some((task_description, branch_name, symbol_ids, file_paths)) = task_history_payload {
        store
            .insert_task_context_history(&TaskContextHistoryRecord {
                task_description,
                branch_name: Some(branch_name),
                resolved_symbol_ids: serde_json::to_string(&symbol_ids)
                    .context("failed to serialize task history symbol ids")?,
                resolved_file_paths: serde_json::to_string(&file_paths)
                    .context("failed to serialize task history file paths")?,
                total_symbols: symbol_ids.len() as i64,
                budget_used: document.budget_usage.used_tokens as i64,
                budget_max: document.budget_usage.max_tokens as i64,
                created_at: current_unix_timestamp_secs(),
            })
            .context("failed to persist task context history")?;
    }
    Ok(())
}

pub fn run_sir_context_command(workspace: &Path, args: SirContextArgs) -> Result<()> {
    let format = parse_legacy_output_format(args.format.as_str())?;
    let include = parse_include_sections(args.include.as_deref())?;
    let selectors = context_selectors(workspace, &args)?;
    let store = SqliteStore::open(workspace).context("failed to open local store")?;

    let mut resolution_errors = Vec::new();
    let mut resolved = Vec::new();
    for selector in &selectors {
        match resolve_symbol(&store, selector) {
            Ok(record) => resolved.push((selector.clone(), record)),
            Err(err) => resolution_errors.push(format!("{selector}: {err}")),
        }
    }
    if !resolution_errors.is_empty() {
        return Err(anyhow!(
            "failed to resolve one or more selectors:\n{}",
            resolution_errors.join("\n")
        ));
    }

    let prepared = resolved
        .iter()
        .map(|(selector, record)| {
            prepare_symbol_context(
                workspace,
                &store,
                selector.as_str(),
                record,
                include,
                args.depth,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let document = if prepared.len() == 1 {
        allocate_single_symbol(prepared, args.max_tokens)?
    } else {
        allocate_batch_symbols(prepared, args.max_tokens)?
    };
    let rendered = render_legacy_document(&document, format);

    if let Some(path) = args.output.as_deref() {
        let path = output_path(path);
        fs::write(&path, rendered)
            .with_context(|| format!("failed to write output file {}", path.display()))?;
        return Ok(());
    }

    let mut out = std::io::stdout();
    out.write_all(rendered.as_bytes())
        .context("failed to write sir-context output")?;
    if !rendered.ends_with('\n') {
        writeln!(&mut out).context("failed to write trailing newline")?;
    }
    Ok(())
}

fn context_targets(workspace: &Path, args: &ContextArgs) -> Result<Vec<ContextTarget>> {
    if args.overview {
        return Ok(Vec::new());
    }

    if let Some(selector) = args
        .symbol
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let file_hint = args
            .file
            .as_deref()
            .map(|value| normalize_workspace_relative_path(workspace, value))
            .transpose()?;
        return Ok(vec![ContextTarget::Symbol {
            selector: selector.to_owned(),
            file_hint,
        }]);
    }

    if args.targets.is_empty() {
        return Err(anyhow!("missing file target, --symbol, or --overview"));
    }

    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for value in &args.targets {
        let normalized = normalize_workspace_relative_path(workspace, value)?;
        if seen.insert(normalized.clone()) {
            targets.push(ContextTarget::File { path: normalized });
        }
    }
    Ok(targets)
}

pub(crate) fn parse_context_format(raw: &str) -> Result<ContextFormat> {
    match raw.trim() {
        "markdown" => Ok(ContextFormat::Markdown),
        "json" => Ok(ContextFormat::Json),
        "xml" => Ok(ContextFormat::Xml),
        "compact" => Ok(ContextFormat::Compact),
        other => Err(anyhow!(
            "unsupported context output format '{other}', expected one of: markdown, json, xml, compact"
        )),
    }
}

pub(crate) fn parse_layer_selection(
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<LayerSelection> {
    let mut layers = match include {
        Some(raw) if !raw.trim().is_empty() => LayerSelection {
            sir: false,
            source: false,
            graph: false,
            coupling: false,
            health: false,
            drift: false,
            memory: false,
            tests: false,
        },
        _ => LayerSelection::all(),
    };

    if let Some(raw) = include {
        for token in split_csv(raw) {
            set_context_layer(&mut layers, token, true)?;
        }
    }
    if let Some(raw) = exclude {
        for token in split_csv(raw) {
            set_context_layer(&mut layers, token, false)?;
        }
    }
    Ok(layers)
}

fn split_csv(raw: &str) -> Vec<&str> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

fn set_context_layer(layers: &mut LayerSelection, token: &str, value: bool) -> Result<()> {
    match token {
        "sir" => layers.sir = value,
        "source" => layers.source = value,
        "graph" => layers.graph = value,
        "coupling" => layers.coupling = value,
        "health" => layers.health = value,
        "drift" => layers.drift = value,
        "memory" => layers.memory = value,
        "tests" => layers.tests = value,
        other => {
            return Err(anyhow!(
                "unsupported context layer '{other}', expected any of: sir, source, graph, coupling, health, drift, memory, tests"
            ));
        }
    }
    Ok(())
}

pub(crate) fn build_project_overview(
    workspace: &Path,
    store: &SqliteStore,
    health_report: Option<&ScoreReport>,
    notices: &mut Vec<String>,
) -> Result<ProjectOverview> {
    let (total_symbols, symbols_with_sir) = store
        .count_symbols_with_sir()
        .context("failed to count indexed symbols")?;
    let sir_coverage_percent = if total_symbols == 0 {
        0.0
    } else {
        (symbols_with_sir as f64 * 100.0) / total_symbols as f64
    };

    let mut overview_notices = Vec::new();
    if total_symbols == 0 {
        overview_notices
            .push("index data unavailable — run `aetherd --index-once` first".to_owned());
    }

    let drift = match store.list_drift_results(false) {
        Ok(rows) => {
            let semantic_findings = rows
                .iter()
                .filter(|row| row.drift_type == "semantic")
                .count();
            let max_magnitude = rows
                .iter()
                .filter_map(|row| row.drift_magnitude)
                .max_by(|left, right| left.total_cmp(right));

            Some(WorkspaceDriftSummary {
                active_findings: rows.len(),
                semantic_findings,
                max_magnitude,
            })
        }
        Err(err) => {
            let message = format!("drift data unavailable — {err}");
            overview_notices.push(message.clone());
            notices.push(message);
            None
        }
    };

    let health = health_report.map(|report| WorkspaceHealthSummary {
        workspace_score: report.workspace_score,
        severity: report.severity.as_label().to_owned(),
        worst_crate: report.worst_crate.clone(),
    });

    Ok(ProjectOverview {
        workspace: workspace.display().to_string(),
        total_symbols,
        symbols_with_sir,
        sir_coverage_percent,
        health,
        drift,
        notices: overview_notices,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_target_section(
    workspace: &Path,
    store: &SqliteStore,
    target: &ContextTarget,
    layers: LayerSelection,
    depth: u32,
    task: Option<&str>,
    health_report: Option<&ScoreReport>,
    context_lines: usize,
) -> Result<PreparedTargetSection> {
    match target {
        ContextTarget::File { path } => prepare_file_target(
            workspace,
            store,
            path.as_str(),
            layers,
            depth,
            task,
            health_report,
            context_lines,
        ),
        ContextTarget::Symbol {
            selector,
            file_hint,
        } => prepare_symbol_target(
            workspace,
            store,
            selector.as_str(),
            file_hint.as_deref(),
            layers,
            depth,
            task,
            health_report,
            context_lines,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_file_target(
    workspace: &Path,
    store: &SqliteStore,
    path: &str,
    layers: LayerSelection,
    depth: u32,
    task: Option<&str>,
    health_report: Option<&ScoreReport>,
    context_lines: usize,
) -> Result<PreparedTargetSection> {
    let full_path = workspace.join(path);
    if !full_path.exists() {
        return Err(anyhow!("file target does not exist: {path}"));
    }

    let file_source = fs::read_to_string(&full_path)
        .with_context(|| format!("failed to read source file {}", full_path.display()))?;
    let language = language_for_path(Path::new(path))
        .map(|value| value.as_str().to_owned())
        .or_else(|| infer_language_name_from_source(path));
    let mut output = TargetSection::file(path.to_owned(), language.clone());

    let mut file_sir = None;
    let mut symbols = Vec::new();
    let mut immediate_graph = Vec::new();
    let mut broader_graph = Vec::new();
    let mut tests = Vec::new();
    let mut coupling = Vec::new();
    let mut memory = Vec::new();
    let mut health = None;
    let mut drift = Vec::new();

    let symbol_records = match store.list_symbols_for_file(path) {
        Ok(records) => records,
        Err(err) => {
            output
                .notices
                .push(format!("index data unavailable for {path} — {err}"));
            Vec::new()
        }
    };

    let graph_store = open_surreal_graph_store_readonly(workspace).ok();

    if symbol_records.is_empty() {
        output
            .notices
            .push(format!("index data unavailable for {path}"));
        if layers.memory {
            memory = prepare_memory_for_file(store, path, task).unwrap_or_default();
        }
        if layers.health {
            health = build_file_health_context(path, &[], health_report, None).map(|value| {
                PreparedItem {
                    cost_text: format!("{}\n{}", value.summary, value.warnings.join("\n")),
                    value,
                }
            });
        }
    } else {
        if layers.sir {
            file_sir = read_file_rollup(store, path, &symbol_records, &mut output.notices)?;
            symbols = symbol_records
                .iter()
                .map(|record| prepare_leaf_symbol_summary(store, record))
                .collect::<Result<Vec<_>>>()?;
            sort_prepared_items(&mut symbols, task);
        }
        if layers.graph {
            immediate_graph = prepare_file_neighbors(store, symbol_records.as_slice(), task)?;
            broader_graph = if depth >= 2 {
                prepare_file_broader_neighbors(store, symbol_records.as_slice(), depth, task)?
            } else {
                Vec::new()
            };
        }
        if layers.tests {
            tests = prepare_tests_for_file(store, path, task)?;
        }
        if layers.coupling {
            let (entries, notice) = prepare_coupling_for_file(graph_store.as_ref(), path)?;
            if let Some(notice) = notice {
                output.notices.push(notice);
            }
            coupling = entries;
        }
        if layers.memory {
            memory = prepare_memory_for_file(store, path, task)?;
        }
        if layers.health {
            let fallback = collect_symbol_health_warnings(store, symbol_records.as_slice());
            health = build_file_health_context(
                path,
                symbol_records.as_slice(),
                health_report,
                Some(fallback),
            )
            .map(|value| PreparedItem {
                cost_text: format!("{}\n{}", value.summary, value.warnings.join("\n")),
                value,
            });
        }
        if layers.drift {
            drift = prepare_drift_for_target(
                store,
                Some(path),
                &symbol_records
                    .iter()
                    .map(|record| record.id.clone())
                    .collect::<Vec<_>>(),
            )?;
        }
    }

    let source = if layers.source {
        Some(prepare_context_source_block(
            workspace,
            path,
            language.as_deref().unwrap_or("text"),
            file_source.as_str(),
            &symbol_records
                .iter()
                .map(|record| record.id.clone())
                .collect::<Vec<_>>(),
            &[],
            depth,
            context_lines,
            &mut output.notices,
        )?)
    } else {
        None
    };

    Ok(PreparedTargetSection {
        output,
        source,
        file_sir,
        symbols,
        immediate_graph,
        broader_graph,
        tests,
        coupling,
        memory,
        health,
        drift,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_symbol_target(
    workspace: &Path,
    store: &SqliteStore,
    selector: &str,
    file_hint: Option<&str>,
    layers: LayerSelection,
    depth: u32,
    task: Option<&str>,
    health_report: Option<&ScoreReport>,
    context_lines: usize,
) -> Result<PreparedTargetSection> {
    let record = resolve_symbol_with_file_hint(store, selector, file_hint)?;
    let include = IncludeSections {
        deps: layers.graph,
        dependents: layers.graph,
        coupling: layers.coupling,
        tests: layers.tests,
        memory: layers.memory,
        changes: false,
        health: layers.health,
    };
    let prepared = prepare_symbol_context(workspace, store, selector, &record, include, depth)?;
    let mut output = TargetSection::symbol(
        selector.to_owned(),
        record.file_path.clone(),
        record.language.clone(),
    );
    output.notices.extend(prepared.base_output.notices.clone());

    let mut tests = prepared.test_guards.clone();
    let mut memory = prepared.memory.clone();
    let mut immediate_graph = prepared
        .dependencies
        .iter()
        .map(|item| PreparedItem {
            cost_text: item.cost_text.clone(),
            value: NeighborSummary {
                relationship: "dependency".to_owned(),
                qualified_name: item.value.qualified_name.clone(),
                file_path: item.value.file_path.clone(),
                intent_summary: item.value.intent_summary.clone(),
                depth: 1,
            },
        })
        .chain(prepared.callers.iter().map(|item| PreparedItem {
            cost_text: item.cost_text.clone(),
            value: NeighborSummary {
                relationship: "caller".to_owned(),
                qualified_name: item.value.qualified_name.clone(),
                file_path: item.value.file_path.clone(),
                intent_summary: "Caller relationship".to_owned(),
                depth: 1,
            },
        }))
        .collect::<Vec<_>>();
    let mut broader_graph = prepared
        .transitive_dependencies
        .iter()
        .map(|item| PreparedItem {
            cost_text: item.cost_text.clone(),
            value: NeighborSummary {
                relationship: "transitive_dependency".to_owned(),
                qualified_name: item.value.qualified_name.clone(),
                file_path: item.value.file_path.clone(),
                intent_summary: item.value.intent_summary.clone(),
                depth: item.value.depth,
            },
        })
        .collect::<Vec<_>>();
    sort_prepared_items(&mut tests, task);
    sort_prepared_items(&mut memory, task);
    sort_prepared_items(&mut immediate_graph, task);
    sort_prepared_items(&mut broader_graph, task);

    let health = if layers.health {
        let fallback = prepared
            .health
            .as_ref()
            .map(|entry| {
                let mut warnings = Vec::new();
                if let Some(staleness) = entry.value.staleness_score {
                    warnings.push(format!("symbol staleness {:.2}", staleness));
                }
                warnings.push(format!(
                    "generation {} via {}",
                    entry.value.generation_pass, entry.value.model
                ));
                warnings.push(format!("SIR status: {}", entry.value.sir_status));
                warnings
            })
            .unwrap_or_default();
        build_file_health_context(
            record.file_path.as_str(),
            std::slice::from_ref(&record),
            health_report,
            Some(fallback),
        )
    } else {
        None
    };

    let drift = if layers.drift {
        prepare_drift_for_target(
            store,
            Some(record.file_path.as_str()),
            std::slice::from_ref(&record.id),
        )?
    } else {
        Vec::new()
    };

    let source = if layers.source {
        let slice_neighbors = collect_same_file_slice_neighbors(store, &record, depth)?;
        let full_source =
            fs::read_to_string(workspace.join(&record.file_path)).with_context(|| {
                format!(
                    "failed to read source file {}",
                    workspace.join(&record.file_path).display()
                )
            })?;
        Some(prepare_context_source_block(
            workspace,
            record.file_path.as_str(),
            prepared.base_output.language.as_str(),
            full_source.as_str(),
            std::slice::from_ref(&record.id),
            slice_neighbors.as_slice(),
            depth,
            context_lines,
            &mut output.notices,
        )?)
    } else {
        None
    };

    let symbols = if layers.sir {
        vec![PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}",
                prepared.base_output.qualified_name,
                prepared.base_output.intent,
                prepared.base_output.behavior.join("\n")
            ),
            value: ExportSymbolContext {
                qualified_name: prepared.base_output.qualified_name.clone(),
                kind: prepared.base_output.kind.clone(),
                file_path: prepared.base_output.file_path.clone(),
                language: prepared.base_output.language.clone(),
                staleness_score: prepared.base_output.staleness_score,
                intent: prepared.base_output.intent.clone(),
                behavior: prepared.base_output.behavior.clone(),
                sir_status: prepared
                    .health
                    .as_ref()
                    .map(|entry| entry.value.sir_status.clone()),
            },
        }]
    } else {
        Vec::new()
    };

    Ok(PreparedTargetSection {
        output,
        source,
        file_sir: None,
        symbols,
        immediate_graph,
        broader_graph,
        tests,
        coupling: prepared.coupling,
        memory,
        health: health.map(|value| PreparedItem {
            cost_text: format!("{}\n{}", value.summary, value.warnings.join("\n")),
            value,
        }),
        drift,
    })
}

#[allow(clippy::too_many_arguments)]
fn prepare_context_source_block(
    workspace: &Path,
    file_path: &str,
    language: &str,
    full_source: &str,
    target_symbol_ids: &[String],
    neighbor_symbol_ids: &[SliceNeighbor],
    depth: u32,
    context_lines: usize,
    notices: &mut Vec<String>,
) -> Result<PreparedItem<SourceBlock>> {
    if target_symbol_ids.is_empty() {
        return Ok(PreparedItem {
            cost_text: full_source.to_owned(),
            value: SourceBlock {
                language: language.to_owned(),
                content: full_source.to_owned(),
            },
        });
    }

    match slice_file_for_context(
        workspace,
        file_path,
        target_symbol_ids,
        neighbor_symbol_ids,
        depth,
        context_lines,
    ) {
        Ok(slice) => {
            let content = render_file_slice(&slice);
            if slice.total_lines >= 50 {
                let included_lines = slice.total_lines.saturating_sub(slice.omitted_lines);
                let saved_tokens =
                    estimate_tokens(full_source).saturating_sub(estimate_tokens(content.as_str()));
                notices.push(format!(
                    "Source sliced: {included_lines} of {} lines included (~{saved_tokens} tokens saved)",
                    slice.total_lines
                ));
            }
            Ok(PreparedItem {
                cost_text: content.clone(),
                value: SourceBlock {
                    language: slice.language,
                    content,
                },
            })
        }
        Err(err) => {
            notices.push(format!(
                "source slicing failed for {file_path} — {err}; including whole file"
            ));
            Ok(PreparedItem {
                cost_text: full_source.to_owned(),
                value: SourceBlock {
                    language: language.to_owned(),
                    content: full_source.to_owned(),
                },
            })
        }
    }
}

fn collect_same_file_slice_neighbors(
    store: &SqliteStore,
    record: &SymbolRecord,
    depth: u32,
) -> Result<Vec<SliceNeighbor>> {
    if depth == 0 {
        return Ok(Vec::new());
    }

    let mut neighbor_depths: HashMap<String, u32> = HashMap::new();
    for edge in store
        .get_callers(record.qualified_name.as_str())
        .with_context(|| format!("failed to list callers for {}", record.qualified_name))?
    {
        let Some(caller) = store
            .get_symbol_record(edge.source_id.as_str())
            .with_context(|| format!("failed to load caller {}", edge.source_id))?
        else {
            continue;
        };
        if caller.file_path == record.file_path && caller.id != record.id {
            neighbor_depths
                .entry(caller.id)
                .and_modify(|existing| *existing = (*existing).min(1))
                .or_insert(1);
        }
    }

    let mut seen = HashSet::new();
    let mut frontier = vec![record.id.clone()];
    for current_depth in 1..=depth {
        let mut next_frontier = Vec::new();
        for source_id in frontier {
            let edges = store
                .get_dependencies(source_id.as_str())
                .with_context(|| format!("failed to list dependencies for {}", source_id))?;
            for edge in edges {
                let Some(target) = store
                    .get_symbol_by_qualified_name(edge.target_qualified_name.as_str())
                    .with_context(|| {
                        format!(
                            "failed to resolve dependency '{}'",
                            edge.target_qualified_name
                        )
                    })?
                else {
                    continue;
                };
                if seen.insert(target.id.clone()) {
                    next_frontier.push(target.id.clone());
                }
                if target.file_path == record.file_path && target.id != record.id {
                    neighbor_depths
                        .entry(target.id)
                        .and_modify(|existing| *existing = (*existing).min(current_depth))
                        .or_insert(current_depth);
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    let mut neighbors = neighbor_depths
        .into_iter()
        .map(|(symbol_id, depth)| SliceNeighbor { symbol_id, depth })
        .collect::<Vec<_>>();
    neighbors.sort_by(|left, right| {
        left.depth
            .cmp(&right.depth)
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });
    Ok(neighbors)
}

fn read_file_rollup(
    store: &SqliteStore,
    file_path: &str,
    symbol_records: &[SymbolRecord],
    notices: &mut Vec<String>,
) -> Result<Option<PreparedItem<FileSirContext>>> {
    let Some(language) = symbol_records
        .first()
        .map(|record| record.language.as_str())
    else {
        return Ok(None);
    };
    let rollup_id = synthetic_file_sir_id(language, file_path);
    let Some(blob) = store
        .read_sir_blob(rollup_id.as_str())
        .with_context(|| format!("failed to read file rollup for {file_path}"))?
    else {
        return Ok(None);
    };

    let file_sir = match serde_json::from_str::<FileSir>(&blob) {
        Ok(value) => value,
        Err(err) => {
            notices.push(format!("file rollup unreadable for {file_path} — {err}"));
            return Ok(None);
        }
    };
    let cost_text = canonicalize_file_sir_json(&file_sir);
    Ok(Some(PreparedItem {
        cost_text,
        value: FileSirContext {
            intent: file_sir.intent,
            exports: file_sir.exports,
            side_effects: file_sir.side_effects,
            dependencies: file_sir.dependencies,
            error_modes: file_sir.error_modes,
            symbol_count: file_sir.symbol_count,
            confidence: file_sir.confidence,
        },
    }))
}

fn prepare_leaf_symbol_summary(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<PreparedItem<ExportSymbolContext>> {
    let meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to read SIR metadata for {}", record.id))?;
    let sir = read_sir_annotation(store, record.id.as_str())
        .with_context(|| format!("failed to read SIR blob for {}", record.id))?;

    let (intent, behavior) = sir_intent_and_behavior(sir.as_ref());
    Ok(PreparedItem {
        cost_text: format!(
            "{}\n{}\n{}",
            record.qualified_name,
            intent,
            behavior.join("\n")
        ),
        value: ExportSymbolContext {
            qualified_name: record.qualified_name.clone(),
            kind: record.kind.clone(),
            file_path: record.file_path.clone(),
            language: record.language.clone(),
            staleness_score: meta.as_ref().and_then(|value| value.staleness_score),
            intent,
            behavior,
            sir_status: meta.map(|value| value.sir_status),
        },
    })
}

fn prepare_file_neighbors(
    store: &SqliteStore,
    symbol_records: &[SymbolRecord],
    task: Option<&str>,
) -> Result<Vec<PreparedItem<NeighborSummary>>> {
    let mut neighbors = Vec::new();
    let mut seen = HashSet::new();

    for record in symbol_records {
        for item in prepare_dependencies(store, record)? {
            let key = format!("dependency::{}", item.value.qualified_name);
            if !seen.insert(key) {
                continue;
            }
            neighbors.push(PreparedItem {
                cost_text: item.cost_text,
                value: NeighborSummary {
                    relationship: "dependency".to_owned(),
                    qualified_name: item.value.qualified_name,
                    file_path: item.value.file_path,
                    intent_summary: item.value.intent_summary,
                    depth: 1,
                },
            });
        }

        let caller_items = prepare_callers_with_intent(store, record)?;
        for item in caller_items {
            let key = format!("caller::{}", item.value.qualified_name);
            if !seen.insert(key) {
                continue;
            }
            neighbors.push(item);
        }
    }

    sort_prepared_items(&mut neighbors, task);
    neighbors.truncate(GRAPH_LIMIT);
    Ok(neighbors)
}

fn prepare_file_broader_neighbors(
    store: &SqliteStore,
    symbol_records: &[SymbolRecord],
    depth: u32,
    task: Option<&str>,
) -> Result<Vec<PreparedItem<NeighborSummary>>> {
    let mut broader = Vec::new();
    let mut seen = HashSet::new();
    for record in symbol_records {
        for item in prepare_transitive_dependencies(store, record, depth)? {
            let key = format!("transitive::{}", item.value.qualified_name);
            if !seen.insert(key) {
                continue;
            }
            broader.push(PreparedItem {
                cost_text: item.cost_text,
                value: NeighborSummary {
                    relationship: "transitive_dependency".to_owned(),
                    qualified_name: item.value.qualified_name,
                    file_path: item.value.file_path,
                    intent_summary: item.value.intent_summary,
                    depth: item.value.depth,
                },
            });
        }
    }
    sort_prepared_items(&mut broader, task);
    broader.truncate(GRAPH_LIMIT);
    Ok(broader)
}

fn prepare_tests_for_file(
    store: &SqliteStore,
    file_path: &str,
    task: Option<&str>,
) -> Result<Vec<PreparedItem<TestGuard>>> {
    let mut prepared = store
        .list_test_intents_for_file(file_path)
        .with_context(|| format!("failed to list test intents for {file_path}"))?
        .into_iter()
        .map(|intent| PreparedItem {
            cost_text: format!("{}\n{}", intent.test_name, intent.intent_text),
            value: TestGuard {
                test_name: intent.test_name,
                description: intent.intent_text,
            },
        })
        .collect::<Vec<_>>();
    sort_prepared_items(&mut prepared, task);
    Ok(prepared)
}

fn prepare_memory_for_file(
    store: &SqliteStore,
    file_path: &str,
    task: Option<&str>,
) -> Result<Vec<PreparedItem<MemoryContext>>> {
    let mut notes = store
        .list_project_notes_for_file_ref(file_path, MEMORY_LIMIT as u32)
        .with_context(|| format!("failed to list project notes for {file_path}"))?;

    if notes.len() < MEMORY_LIMIT {
        let remaining = MEMORY_LIMIT.saturating_sub(notes.len()) as u32;
        let query = task
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| file_stem(file_path));
        let extra = store
            .search_project_notes_lexical(query.as_str(), remaining, false, &[])
            .with_context(|| format!("failed to search project notes for '{query}'"))?;
        let mut seen = notes
            .iter()
            .map(|note| note.note_id.clone())
            .collect::<HashSet<_>>();
        for note in extra {
            if seen.insert(note.note_id.clone()) {
                notes.push(note);
            }
        }
    }

    let mut prepared = notes
        .into_iter()
        .map(|note| PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}",
                first_line(note.content.as_str()),
                note.source_type,
                note.created_at
            ),
            value: MemoryContext {
                first_line: first_line(note.content.as_str()),
                source_type: note.source_type,
                created_at: note.created_at,
            },
        })
        .collect::<Vec<_>>();
    sort_prepared_items(&mut prepared, task);
    prepared.truncate(MEMORY_LIMIT);
    Ok(prepared)
}

fn build_file_health_context(
    file_path: &str,
    symbols: &[SymbolRecord],
    report: Option<&ScoreReport>,
    fallback_warnings: Option<Vec<String>>,
) -> Option<ExportHealthContext> {
    let mut warnings = fallback_warnings.unwrap_or_default();
    let mut summary = None;

    if let Some(report) = report {
        let crate_name = crate_name_for_file(file_path, report);
        if let Some(crate_name) = crate_name
            && let Some(crate_score) = report.crates.iter().find(|entry| entry.name == crate_name)
        {
            summary = Some(format!(
                "crate {} scored {}/100 ({})",
                crate_score.name,
                crate_score.score,
                crate_score.severity.as_label()
            ));
            warnings.extend(
                crate_score
                    .violations
                    .iter()
                    .take(3)
                    .map(|violation| format!("{}: {}", violation.metric, violation.reason)),
            );
        }
    }

    if summary.is_none() && !symbols.is_empty() {
        let stale_count = symbols
            .iter()
            .filter(|record| {
                report_symbol_status(record)
                    .is_some_and(|status| status.eq_ignore_ascii_case("stale"))
            })
            .count();
        summary = Some(format!(
            "{} indexed symbol(s) in {}",
            symbols.len(),
            file_path
        ));
        if stale_count > 0 {
            warnings.push(format!("{stale_count} symbol(s) have stale SIR metadata"));
        }
    }

    let summary = summary.or_else(|| {
        if warnings.is_empty() {
            None
        } else {
            Some(format!("health summary for {file_path}"))
        }
    })?;

    warnings.sort();
    warnings.dedup();
    Some(ExportHealthContext { summary, warnings })
}

fn report_symbol_status(_record: &SymbolRecord) -> Option<String> {
    None
}

fn collect_symbol_health_warnings(store: &SqliteStore, symbols: &[SymbolRecord]) -> Vec<String> {
    let mut warnings = Vec::new();
    for symbol in symbols {
        if let Ok(Some(meta)) = store.get_sir_meta(symbol.id.as_str())
            && meta.sir_status.trim().eq_ignore_ascii_case("stale")
        {
            warnings.push(format!("{} has stale SIR", symbol.qualified_name));
        }
    }
    warnings
}

fn crate_name_for_file(file_path: &str, report: &ScoreReport) -> Option<String> {
    let normalized = normalize_path(file_path);
    if let Some(crate_name) = normalized
        .strip_prefix("crates/")
        .and_then(|rest| rest.split('/').next())
        .map(str::to_owned)
        && report.crates.iter().any(|entry| entry.name == crate_name)
    {
        return Some(crate_name);
    }

    if report.crates.len() == 1 {
        return report.crates.first().map(|entry| entry.name.clone());
    }
    None
}

fn prepare_drift_for_target(
    store: &SqliteStore,
    file_path: Option<&str>,
    symbol_ids: &[String],
) -> Result<Vec<PreparedItem<DriftContext>>> {
    let symbol_id_set = symbol_ids
        .iter()
        .map(|value| value.as_str())
        .collect::<HashSet<_>>();
    let mut rows = store
        .list_drift_results(false)
        .context("failed to list drift results")?
        .into_iter()
        .filter(|row| {
            file_path.is_some_and(|path| row.file_path == path)
                || symbol_id_set.contains(row.symbol_id.as_str())
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .drift_magnitude
            .unwrap_or_default()
            .total_cmp(&left.drift_magnitude.unwrap_or_default())
            .then_with(|| right.detected_at.cmp(&left.detected_at))
    });

    Ok(rows
        .into_iter()
        .take(DRIFT_LIMIT)
        .map(|row| PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}\n{}",
                row.symbol_name,
                row.drift_type,
                row.drift_magnitude.unwrap_or_default(),
                row.drift_summary.clone().unwrap_or_default()
            ),
            value: DriftContext {
                symbol_name: row.symbol_name,
                drift_type: row.drift_type,
                drift_magnitude: row.drift_magnitude,
                summary: row
                    .drift_summary
                    .unwrap_or_else(|| "drift detected".to_owned()),
                detected_at: row.detected_at,
            },
        })
        .collect())
}

pub(crate) fn allocate_export_document(
    overview: ProjectOverview,
    prepared: Vec<PreparedTargetSection>,
    max_tokens: usize,
    notices: Vec<String>,
) -> ExportDocument {
    let mut prepared = prepared;
    let mut sections = prepared
        .iter()
        .map(|section| section.output.clone())
        .collect::<Vec<_>>();
    let mut budget = BudgetAllocator::new(max_tokens);
    let layers = vec![
        allocate_source_layer(&mut budget, &mut sections, &mut prepared, max_tokens),
        allocate_sir_layer(&mut budget, &mut sections, &mut prepared, max_tokens),
        allocate_vec_layer(
            "graph",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.immediate_graph,
            |prepared| &mut prepared.immediate_graph,
            "graph neighbors",
        ),
        allocate_vec_layer(
            "tests",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.tests,
            |prepared| &mut prepared.tests,
            "tests",
        ),
        allocate_vec_layer(
            "coupling",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.coupling,
            |prepared| &mut prepared.coupling,
            "coupling entries",
        ),
        allocate_vec_layer(
            "memory",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.memory,
            |prepared| &mut prepared.memory,
            "memory notes",
        ),
        allocate_optional_layer(
            "health",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.health,
            |prepared| &mut prepared.health,
            "health summary",
        ),
        allocate_vec_layer(
            "drift",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.drift,
            |prepared| &mut prepared.drift,
            "drift findings",
        ),
        allocate_vec_layer(
            "broader_graph",
            &mut budget,
            &mut sections,
            &mut prepared,
            max_tokens,
            |section| &mut section.broader_graph,
            |prepared| &mut prepared.broader_graph,
            "broader graph entries",
        ),
    ];

    ExportDocument {
        generated_at: current_unix_timestamp_secs(),
        project_overview: overview,
        target_sections: sections,
        budget_usage: BudgetUsage {
            max_tokens,
            used_tokens: budget.used_tokens,
            layers,
        },
        notices,
    }
}

fn allocate_source_layer(
    budget: &mut BudgetAllocator,
    sections: &mut [TargetSection],
    prepared: &mut [PreparedTargetSection],
    max_tokens: usize,
) -> LayerBudgetLine {
    let mut stats = LayerBudgetStats::default();
    for (index, section) in sections.iter_mut().enumerate() {
        let Some(item) = prepared[index].source.take() else {
            continue;
        };
        let included = budget.try_add(item.cost_text.as_str());
        stats.note_attempt(item.cost_text.as_str(), included);
        if included {
            section.source = Some(item.value);
        } else {
            section
                .notices
                .push("source omitted to fit the requested context budget".to_owned());
        }
    }
    stats.finish("source", max_tokens, suggested_pct("source"))
}

fn allocate_sir_layer(
    budget: &mut BudgetAllocator,
    sections: &mut [TargetSection],
    prepared: &mut [PreparedTargetSection],
    max_tokens: usize,
) -> LayerBudgetLine {
    let mut stats = LayerBudgetStats::default();
    for (index, section) in sections.iter_mut().enumerate() {
        if let Some(item) = prepared[index].file_sir.take() {
            let included = budget.try_add(item.cost_text.as_str());
            stats.note_attempt(item.cost_text.as_str(), included);
            if included {
                section.file_sir = Some(item.value);
            } else {
                section
                    .notices
                    .push("file rollup omitted to fit the requested context budget".to_owned());
            }
        }

        let symbol_items = std::mem::take(&mut prepared[index].symbols);
        if let Some(notice) = allocate_prepared_vec(
            budget,
            &mut stats,
            &mut section.symbols,
            symbol_items,
            "symbol summaries",
        ) {
            section.notices.push(notice);
        }
    }
    stats.finish("sir", max_tokens, suggested_pct("sir"))
}

#[allow(clippy::too_many_arguments)]
fn allocate_vec_layer<T: Clone>(
    layer: &str,
    budget: &mut BudgetAllocator,
    sections: &mut [TargetSection],
    prepared: &mut [PreparedTargetSection],
    max_tokens: usize,
    target: fn(&mut TargetSection) -> &mut Vec<T>,
    source: fn(&mut PreparedTargetSection) -> &mut Vec<PreparedItem<T>>,
    label: &str,
) -> LayerBudgetLine {
    let mut stats = LayerBudgetStats::default();
    for (index, section) in sections.iter_mut().enumerate() {
        let items = std::mem::take(source(&mut prepared[index]));
        let notice = {
            let target_items = target(section);
            allocate_prepared_vec(budget, &mut stats, target_items, items, label)
        };
        if let Some(notice) = notice {
            section.notices.push(notice);
        }
    }
    stats.finish(layer, max_tokens, suggested_pct(layer))
}

#[allow(clippy::too_many_arguments)]
fn allocate_optional_layer<T: Clone>(
    layer: &str,
    budget: &mut BudgetAllocator,
    sections: &mut [TargetSection],
    prepared: &mut [PreparedTargetSection],
    max_tokens: usize,
    target: fn(&mut TargetSection) -> &mut Option<T>,
    source: fn(&mut PreparedTargetSection) -> &mut Option<PreparedItem<T>>,
    label: &str,
) -> LayerBudgetLine {
    let mut stats = LayerBudgetStats::default();
    for (index, section) in sections.iter_mut().enumerate() {
        let Some(item) = source(&mut prepared[index]).take() else {
            continue;
        };
        let included = budget.try_add(item.cost_text.as_str());
        stats.note_attempt(item.cost_text.as_str(), included);
        if included {
            *target(section) = Some(item.value);
        } else {
            section.notices.push(format!(
                "{label} omitted to fit the requested context budget"
            ));
        }
    }
    stats.finish(layer, max_tokens, suggested_pct(layer))
}

fn allocate_prepared_vec<T: Clone>(
    budget: &mut BudgetAllocator,
    stats: &mut LayerBudgetStats,
    target: &mut Vec<T>,
    items: Vec<PreparedItem<T>>,
    label: &str,
) -> Option<String> {
    for (index, item) in items.iter().enumerate() {
        let included = budget.try_add(item.cost_text.as_str());
        stats.note_attempt(item.cost_text.as_str(), included);
        if included {
            target.push(item.value.clone());
            continue;
        }

        let omitted = items.len().saturating_sub(index);
        if omitted > 0 {
            let notice = format!("{omitted} {label} omitted to fit the requested context budget");
            stats.attempted_items = stats
                .attempted_items
                .saturating_add(omitted.saturating_sub(1));
            return Some(notice);
        }
        break;
    }
    None
}

pub(crate) fn render_export_document(document: &ExportDocument, format: ContextFormat) -> String {
    match format {
        ContextFormat::Markdown => render_export_markdown(document),
        ContextFormat::Json => {
            serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned())
        }
        ContextFormat::Xml => context_renderers::render_xml(document),
        ContextFormat::Compact => context_renderers::render_compact(document),
    }
}

fn render_export_markdown(document: &ExportDocument) -> String {
    let mut out = String::new();
    out.push_str("# AETHER Context\n\n");
    out.push_str(&format!(
        "Generated: {} | Budget: {} tokens | Used: {} tokens\n\n",
        document.generated_at, document.budget_usage.max_tokens, document.budget_usage.used_tokens
    ));

    out.push_str("## Project Overview\n");
    out.push_str(&format!(
        "- Workspace: `{}`\n- Total Symbols: {} | SIR Coverage: {:.1}%\n",
        document.project_overview.workspace,
        document.project_overview.total_symbols,
        document.project_overview.sir_coverage_percent
    ));
    if let Some(health) = &document.project_overview.health {
        out.push_str(&format!(
            "- Health Score: {}/100 ({})\n",
            health.workspace_score, health.severity
        ));
        if let Some(worst) = health.worst_crate.as_deref() {
            out.push_str(&format!("- Worst Crate: `{worst}`\n"));
        }
    }
    if let Some(drift) = &document.project_overview.drift {
        out.push_str(&format!(
            "- Active Drift: {} finding(s), {} semantic, max magnitude {}\n",
            drift.active_findings,
            drift.semantic_findings,
            drift
                .max_magnitude
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_owned())
        ));
    }
    for notice in &document.project_overview.notices {
        out.push_str(&format!("> [{notice}]\n"));
    }
    out.push('\n');

    for section in &document.target_sections {
        if section.target_kind == "file" {
            out.push_str(&format!("## Target File: {}\n\n", section.target_label));
        } else {
            out.push_str(&format!("## Target Symbol: {}\n\n", section.target_label));
        }

        if let Some(file_sir) = &section.file_sir {
            out.push_str("### File Rollup\n");
            out.push_str(&format!(
                "- Confidence: {:.2}\n- Symbol Count: {}\n\n**Intent:** {}\n\n",
                file_sir.confidence, file_sir.symbol_count, file_sir.intent
            ));
            render_markdown_list(
                &mut out,
                "#### Exports",
                file_sir.exports.iter().map(|item| format!("- `{item}`")),
            );
            render_markdown_list(
                &mut out,
                "#### Side Effects",
                file_sir.side_effects.iter().map(|item| format!("- {item}")),
            );
            render_markdown_list(
                &mut out,
                "#### Dependencies",
                file_sir.dependencies.iter().map(|item| format!("- {item}")),
            );
            render_markdown_list(
                &mut out,
                "#### Error Modes",
                file_sir.error_modes.iter().map(|item| format!("- {item}")),
            );
        }

        if !section.symbols.is_empty() {
            out.push_str(&format!("### Symbols ({})\n\n", section.symbols.len()));
            for symbol in &section.symbols {
                out.push_str(&format!(
                    "#### `{}` ({})\n**Intent:** {}\n",
                    symbol.qualified_name, symbol.kind, symbol.intent
                ));
                if !symbol.behavior.is_empty() {
                    out.push_str("**Behavior:**\n");
                    for entry in &symbol.behavior {
                        out.push_str(&format!("- {entry}\n"));
                    }
                }
                if let Some(status) = symbol.sir_status.as_deref() {
                    out.push_str(&format!("**SIR Status:** {status}\n"));
                }
                if let Some(staleness) = symbol.staleness_score {
                    out.push_str(&format!("**Staleness:** {staleness:.2}\n"));
                }
                out.push('\n');
            }
        }

        if let Some(source) = &section.source {
            out.push_str("### Source\n");
            out.push_str(&format!(
                "```{}\n{}\n```\n\n",
                source.language, source.content
            ));
        }

        render_markdown_list(
            &mut out,
            "### Dependency Neighborhood",
            section.immediate_graph.iter().map(|neighbor| {
                format!(
                    "- depth {} {} `{}` ({}) — {}",
                    neighbor.depth,
                    neighbor.relationship,
                    neighbor.qualified_name,
                    neighbor.file_path,
                    neighbor.intent_summary
                )
            }),
        );
        render_markdown_list(
            &mut out,
            "### Broader Graph",
            section.broader_graph.iter().map(|neighbor| {
                format!(
                    "- depth {} `{}` ({}) — {}",
                    neighbor.depth,
                    neighbor.qualified_name,
                    neighbor.file_path,
                    neighbor.intent_summary
                )
            }),
        );
        render_markdown_list(
            &mut out,
            "### Test Intents",
            section
                .tests
                .iter()
                .map(|guard| format!("- `{}` — {}", guard.test_name, guard.description)),
        );
        render_markdown_list(
            &mut out,
            "### Coupling",
            section
                .coupling
                .iter()
                .map(|entry| format!("- `{}` — fused {:.2}", entry.file_path, entry.fused_score)),
        );
        render_markdown_list(
            &mut out,
            "### Relevant Memory",
            section.memory.iter().map(|note| {
                format!(
                    "- {} ({}, {})",
                    note.first_line,
                    note.source_type,
                    format_relative_age(note.created_at)
                )
            }),
        );
        if let Some(health) = &section.health {
            out.push_str("### Health\n");
            out.push_str(&format!("- {}\n", health.summary));
            for warning in &health.warnings {
                out.push_str(&format!("- {warning}\n"));
            }
            out.push('\n');
        }
        render_markdown_list(
            &mut out,
            "### Active Drift",
            section.drift.iter().map(|entry| {
                format!(
                    "- `{}` {} {} — {}",
                    entry.symbol_name,
                    entry.drift_type,
                    entry
                        .drift_magnitude
                        .map(|value| format!("{value:.2}"))
                        .unwrap_or_else(|| "n/a".to_owned()),
                    entry.summary
                )
            }),
        );
        for notice in &section.notices {
            out.push_str(&format!("> [{notice}]\n"));
        }
        out.push('\n');
    }

    for notice in &document.notices {
        out.push_str(&format!("> [{notice}]\n"));
    }
    out.push('\n');
    out.push_str("## Budget Usage\n");
    out.push_str("| Layer | Suggested | Used | Status |\n");
    out.push_str("| --- | ---: | ---: | --- |\n");
    for line in &document.budget_usage.layers {
        out.push_str(&format!(
            "| {} | {} | {} | {:?} |\n",
            line.layer, line.suggested_tokens, line.used_tokens, line.status
        ));
    }
    out
}

fn suggested_pct(layer: &str) -> usize {
    LAYER_SUGGESTIONS
        .iter()
        .find(|(name, _)| *name == layer)
        .map(|(_, pct)| *pct)
        .unwrap_or_default()
}

fn estimate_tokens(content: &str) -> usize {
    ((content.len() as f64) / CHARS_PER_TOKEN).ceil() as usize
}

fn sort_prepared_items<T>(items: &mut [PreparedItem<T>], task: Option<&str>) {
    items.sort_by(|left, right| {
        let left_score = task_relevance_score(task, left.cost_text.as_str());
        let right_score = task_relevance_score(task, right.cost_text.as_str());
        right_score
            .cmp(&left_score)
            .then_with(|| left.cost_text.cmp(&right.cost_text))
    });
}

fn task_relevance_score(task: Option<&str>, text: &str) -> usize {
    let Some(task) = task.map(str::trim).filter(|value| !value.is_empty()) else {
        return 0;
    };

    let haystack = text.to_ascii_lowercase();
    task.split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .filter(|term| term.len() >= 3)
        .map(|term| haystack.matches(&term.to_ascii_lowercase()).count())
        .sum()
}

fn normalize_workspace_relative_path(workspace: &Path, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("path must not be empty"));
    }

    let path = PathBuf::from(trimmed);
    // Reject parent traversal components before any resolution
    if path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(anyhow!("path must not contain '..' components"));
    }
    let normalized = if path.is_absolute() {
        if !path.starts_with(workspace) {
            return Err(anyhow!(
                "path must be under workspace {}",
                workspace.display()
            ));
        }
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| anyhow!("path must be under workspace {}", workspace.display()))?;
        normalize_path(relative.to_string_lossy().as_ref())
    } else {
        normalize_path(trimmed)
    };

    let mut normalized = normalized.trim().to_owned();
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_owned();
    }
    if normalized != "/" {
        normalized = normalized.trim_end_matches('/').to_owned();
    }
    if normalized.is_empty() {
        return Err(anyhow!("path must not be empty"));
    }
    Ok(normalized)
}

fn resolve_symbol_with_file_hint(
    store: &SqliteStore,
    selector: &str,
    file_hint: Option<&str>,
) -> Result<SymbolRecord> {
    let Some(file_hint) = file_hint.map(normalize_path) else {
        return resolve_symbol(store, selector);
    };

    let selector = selector.trim();
    if selector.is_empty() {
        return Err(anyhow!("symbol selector must not be empty"));
    }

    let mut candidates = Vec::new();
    if let Some(record) = store
        .get_symbol_record(selector)
        .with_context(|| format!("failed to look up symbol id '{selector}'"))?
        && normalize_path(record.file_path.as_str()) == file_hint
    {
        candidates.push(record);
    }
    if let Some(record) = store
        .get_symbol_by_qualified_name(selector)
        .with_context(|| format!("failed to look up qualified name '{selector}'"))?
        && normalize_path(record.file_path.as_str()) == file_hint
        && !candidates.iter().any(|candidate| candidate.id == record.id)
    {
        candidates.push(record);
    }
    let search = store
        .search_symbols(selector, 25)
        .with_context(|| format!("failed to search symbols for '{selector}'"))?;
    for candidate in search {
        if normalize_path(candidate.file_path.as_str()) != file_hint {
            continue;
        }
        let Some(record) = store
            .get_symbol_record(candidate.symbol_id.as_str())
            .with_context(|| format!("failed to load symbol record for {}", candidate.symbol_id))?
        else {
            continue;
        };
        if !candidates.iter().any(|existing| existing.id == record.id) {
            candidates.push(record);
        }
    }

    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(anyhow!(
            "symbol '{selector}' was not found in file hint '{file_hint}'"
        )),
        _ => {
            let options = candidates
                .iter()
                .map(|record| format!("{} [{}]", record.qualified_name, record.file_path))
                .collect::<Vec<_>>()
                .join("\n  - ");
            Err(anyhow!(
                "ambiguous symbol selector '{selector}' in '{file_hint}'. Candidates:\n  - {options}"
            ))
        }
    }
}

fn language_for_path(path: &Path) -> Option<Language> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("rs") => Some(Language::Rust),
        Some("ts") => Some(Language::TypeScript),
        Some("tsx") => Some(Language::Tsx),
        Some("js") => Some(Language::JavaScript),
        Some("jsx") => Some(Language::Jsx),
        Some("py") => Some(Language::Python),
        _ => None,
    }
}

fn infer_language_name_from_source(path: &str) -> Option<String> {
    language_for_path(Path::new(path)).map(|value| value.as_str().to_owned())
}

fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_owned()
}

fn compute_workspace_health_report(workspace: &Path) -> Result<Option<ScoreReport>> {
    let config = workspace_health_config_or_default(workspace);
    match aether_health::compute_workspace_score(workspace, &config) {
        Ok(report) => Ok(Some(report)),
        Err(err) => {
            if err.to_string().trim().is_empty() {
                Ok(None)
            } else {
                Err(anyhow!(err.to_string()))
            }
        }
    }
}

fn read_sir_annotation(store: &SqliteStore, symbol_id: &str) -> Result<Option<SirAnnotation>> {
    store
        .read_sir_blob(symbol_id)?
        .map(|blob| serde_json::from_str::<SirAnnotation>(&blob))
        .transpose()
        .map_err(Into::into)
}

fn sir_intent_and_behavior(sir: Option<&SirAnnotation>) -> (String, Vec<String>) {
    match sir {
        Some(sir) => {
            let mut behavior = Vec::new();
            behavior.extend(sir.side_effects.iter().cloned());
            behavior.extend(sir.error_modes.iter().cloned());
            (sir.intent.clone(), behavior)
        }
        None => ("No SIR recorded.".to_owned(), Vec::new()),
    }
}

// Legacy sir-context compatibility layer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyOutputFormat {
    Markdown,
    Json,
    Text,
}

#[derive(Debug, Clone, Copy)]
struct IncludeSections {
    deps: bool,
    dependents: bool,
    coupling: bool,
    tests: bool,
    memory: bool,
    changes: bool,
    health: bool,
}

impl IncludeSections {
    fn all() -> Self {
        Self {
            deps: true,
            dependents: true,
            coupling: true,
            tests: true,
            memory: true,
            changes: true,
            health: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct LegacyContextDocument {
    symbols: Vec<LegacySymbolContext>,
    used_tokens: usize,
    max_tokens: usize,
    notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LegacySymbolContext {
    selector: String,
    qualified_name: String,
    kind: String,
    file_path: String,
    language: String,
    staleness_score: Option<f64>,
    source_code: String,
    intent: String,
    behavior: Vec<String>,
    test_guards: Vec<TestGuard>,
    dependencies: Vec<DependencyContext>,
    callers: Vec<CallerContext>,
    coupling: Vec<CouplingContext>,
    memory: Vec<MemoryContext>,
    recent_changes: Vec<ChangeContext>,
    health: Option<HealthContext>,
    transitive_dependencies: Vec<TransitiveDependencyContext>,
    notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DependencyContext {
    qualified_name: String,
    file_path: String,
    intent_summary: String,
}

#[derive(Debug, Clone, Serialize)]
struct CallerContext {
    qualified_name: String,
    file_path: String,
}

#[derive(Debug, Clone, Serialize)]
struct ChangeContext {
    relative_age: String,
    message: String,
    short_sha: String,
}

#[derive(Debug, Clone, Serialize)]
struct HealthContext {
    staleness_score: Option<f64>,
    generation_pass: String,
    model: String,
    updated_at: i64,
    sir_status: String,
}

#[derive(Debug, Clone, Serialize)]
struct TransitiveDependencyContext {
    qualified_name: String,
    file_path: String,
    intent_summary: String,
    depth: u32,
}

#[derive(Debug, Clone)]
struct PreparedSymbolContext {
    base_output: LegacySymbolContext,
    base_cost: String,
    test_guards: Vec<PreparedItem<TestGuard>>,
    dependencies: Vec<PreparedItem<DependencyContext>>,
    callers: Vec<PreparedItem<CallerContext>>,
    coupling: Vec<PreparedItem<CouplingContext>>,
    memory: Vec<PreparedItem<MemoryContext>>,
    recent_changes: Vec<PreparedItem<ChangeContext>>,
    health: Option<PreparedItem<HealthContext>>,
    transitive_dependencies: Vec<PreparedItem<TransitiveDependencyContext>>,
}

fn context_selectors(workspace: &Path, args: &SirContextArgs) -> Result<Vec<String>> {
    if let Some(path) = args.symbols.as_deref() {
        return read_selector_file(&workspace.join(path));
    }

    match args.selector.as_deref().map(str::trim) {
        Some(selector) if !selector.is_empty() => Ok(vec![selector.to_owned()]),
        _ => Err(anyhow!("missing symbol selector")),
    }
}

fn prepare_symbol_context(
    workspace: &Path,
    store: &SqliteStore,
    selector: &str,
    record: &SymbolRecord,
    include: IncludeSections,
    depth: u32,
) -> Result<PreparedSymbolContext> {
    let fresh = load_fresh_symbol_source(workspace, record)?;
    let meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to read SIR metadata for {}", record.id))?;
    let sir = read_sir_annotation(store, record.id.as_str())
        .with_context(|| format!("failed to parse SIR JSON for {}", record.id))?;

    let (intent, behavior) = sir_intent_and_behavior(sir.as_ref());

    let mut base_output = LegacySymbolContext {
        selector: selector.to_owned(),
        qualified_name: record.qualified_name.clone(),
        kind: record.kind.clone(),
        file_path: record.file_path.clone(),
        language: record.language.clone(),
        staleness_score: meta.as_ref().and_then(|value| value.staleness_score),
        source_code: fresh.symbol_source.clone(),
        intent: intent.clone(),
        behavior: behavior.clone(),
        test_guards: Vec::new(),
        dependencies: Vec::new(),
        callers: Vec::new(),
        coupling: Vec::new(),
        memory: Vec::new(),
        recent_changes: Vec::new(),
        health: None,
        transitive_dependencies: Vec::new(),
        notices: Vec::new(),
    };
    let base_cost = format!(
        "{}\n{}\n{}",
        fresh.symbol_source,
        intent,
        behavior.join("\n")
    );

    let test_guards = if include.tests {
        prepare_test_guards(store, record)?
    } else {
        Vec::new()
    };
    let dependencies = if include.deps {
        prepare_dependencies(store, record)?
    } else {
        Vec::new()
    };
    let callers = if include.dependents {
        prepare_callers(store, record)?
    } else {
        Vec::new()
    };
    let graph_store = open_surreal_graph_store_readonly(workspace).ok();
    let coupling = if include.coupling {
        let (entries, notice) =
            prepare_coupling_for_file(graph_store.as_ref(), record.file_path.as_str())?;
        if let Some(notice) = notice {
            base_output.notices.push(notice);
        }
        entries
    } else {
        Vec::new()
    };
    let memory = if include.memory {
        prepare_memory(store, record)?
    } else {
        Vec::new()
    };
    let recent_changes = if include.changes {
        prepare_recent_changes(workspace, record)
    } else {
        Vec::new()
    };
    let health = if include.health {
        meta.as_ref().map(|entry| PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}\n{}\n{}",
                entry.staleness_score.unwrap_or_default(),
                entry.generation_pass,
                entry.model,
                entry.updated_at,
                entry.sir_status
            ),
            value: HealthContext {
                staleness_score: entry.staleness_score,
                generation_pass: entry.generation_pass.clone(),
                model: entry.model.clone(),
                updated_at: entry.updated_at,
                sir_status: entry.sir_status.clone(),
            },
        })
    } else {
        None
    };
    let transitive_dependencies = if include.deps && depth >= 2 {
        prepare_transitive_dependencies(store, record, depth)?
    } else {
        Vec::new()
    };

    Ok(PreparedSymbolContext {
        base_output,
        base_cost,
        test_guards,
        dependencies,
        callers,
        coupling,
        memory,
        recent_changes,
        health,
        transitive_dependencies,
    })
}

fn prepare_test_guards(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<Vec<PreparedItem<TestGuard>>> {
    let direct = store
        .list_test_intents_for_symbol(record.id.as_str())
        .with_context(|| format!("failed to list test intents for {}", record.id))?;
    let intents = if direct.is_empty() {
        store
            .list_test_intents_for_file(record.file_path.as_str())
            .with_context(|| format!("failed to list test intents for {}", record.file_path))?
    } else {
        direct
    };
    Ok(intents
        .into_iter()
        .map(|intent| PreparedItem {
            cost_text: format!("{}\n{}", intent.test_name, intent.intent_text),
            value: TestGuard {
                test_name: intent.test_name,
                description: intent.intent_text,
            },
        })
        .collect())
}

fn prepare_dependencies(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<Vec<PreparedItem<DependencyContext>>> {
    let edges = store
        .get_dependencies(record.id.as_str())
        .with_context(|| format!("failed to list dependencies for {}", record.id))?;
    let mut prepared = Vec::new();
    let mut seen = HashSet::new();
    for edge in edges {
        if !seen.insert(edge.target_qualified_name.clone()) {
            continue;
        }
        let Some(target) = store
            .get_symbol_by_qualified_name(edge.target_qualified_name.as_str())
            .with_context(|| {
                format!(
                    "failed to resolve dependency '{}'",
                    edge.target_qualified_name
                )
            })?
        else {
            continue;
        };
        let summary = read_intent_summary(store, target.id.as_str())
            .unwrap_or_else(|| "No SIR recorded.".to_owned());
        prepared.push(PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}",
                target.qualified_name, target.file_path, summary
            ),
            value: DependencyContext {
                qualified_name: target.qualified_name,
                file_path: target.file_path,
                intent_summary: summary,
            },
        });
    }
    Ok(prepared)
}

fn prepare_callers(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<Vec<PreparedItem<CallerContext>>> {
    let edges = store
        .get_callers(record.qualified_name.as_str())
        .with_context(|| format!("failed to list callers for {}", record.qualified_name))?;
    let mut prepared = Vec::new();
    let mut seen = HashSet::new();
    for edge in edges {
        let Some(caller) = store
            .get_symbol_record(edge.source_id.as_str())
            .with_context(|| format!("failed to load caller {}", edge.source_id))?
        else {
            continue;
        };
        if !seen.insert(caller.id.clone()) {
            continue;
        }
        prepared.push(PreparedItem {
            cost_text: format!("{}\n{}", caller.qualified_name, caller.file_path),
            value: CallerContext {
                qualified_name: caller.qualified_name,
                file_path: caller.file_path,
            },
        });
    }
    Ok(prepared)
}

fn prepare_callers_with_intent(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<Vec<PreparedItem<NeighborSummary>>> {
    let edges = store
        .get_callers(record.qualified_name.as_str())
        .with_context(|| format!("failed to list callers for {}", record.qualified_name))?;
    let mut prepared = Vec::new();
    let mut seen = HashSet::new();
    for edge in edges {
        let Some(caller) = store
            .get_symbol_record(edge.source_id.as_str())
            .with_context(|| format!("failed to load caller {}", edge.source_id))?
        else {
            continue;
        };
        if !seen.insert(caller.id.clone()) {
            continue;
        }
        let summary = read_intent_summary(store, caller.id.as_str())
            .unwrap_or_else(|| "Caller relationship".to_owned());
        prepared.push(PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}",
                caller.qualified_name, caller.file_path, summary
            ),
            value: NeighborSummary {
                relationship: "caller".to_owned(),
                qualified_name: caller.qualified_name,
                file_path: caller.file_path,
                intent_summary: summary,
                depth: 1,
            },
        });
    }
    Ok(prepared)
}

fn prepare_coupling_for_file(
    graph_store: Option<&SurrealGraphStore>,
    file_path: &str,
) -> Result<(Vec<PreparedItem<CouplingContext>>, Option<String>)> {
    let Some(graph_store) = graph_store else {
        return Ok((
            Vec::new(),
            Some("coupling data unavailable — daemon may hold SurrealDB lock".to_owned()),
        ));
    };
    let edges =
        match block_on_store_future(graph_store.list_co_change_edges_for_file(file_path, 0.0)) {
            Ok(Ok(edges)) => edges,
            Err(_) => {
                return Ok((
                    Vec::new(),
                    Some("coupling data unavailable — daemon may hold SurrealDB lock".to_owned()),
                ));
            }
            Ok(Err(_)) => {
                return Ok((
                    Vec::new(),
                    Some("coupling data unavailable — daemon may hold SurrealDB lock".to_owned()),
                ));
            }
        };
    Ok((
        edges
            .into_iter()
            .map(|edge| coupling_entry(file_path, edge))
            .collect(),
        None,
    ))
}

fn prepare_memory(
    store: &SqliteStore,
    record: &SymbolRecord,
) -> Result<Vec<PreparedItem<MemoryContext>>> {
    let mut notes = store
        .list_project_notes_for_file_ref(record.file_path.as_str(), MEMORY_LIMIT as u32)
        .with_context(|| format!("failed to list project notes for {}", record.file_path))?;
    if notes.len() < MEMORY_LIMIT {
        let remaining = MEMORY_LIMIT.saturating_sub(notes.len()) as u32;
        let query = record
            .qualified_name
            .rsplit("::")
            .next()
            .or_else(|| record.qualified_name.rsplit('.').next())
            .unwrap_or(record.qualified_name.as_str());
        let extra = store
            .search_project_notes_lexical(query, remaining, false, &[])
            .with_context(|| format!("failed to search project notes for '{query}'"))?;
        let mut seen = notes
            .iter()
            .map(|note| note.note_id.clone())
            .collect::<HashSet<_>>();
        for note in extra {
            if seen.insert(note.note_id.clone()) {
                notes.push(note);
            }
        }
    }
    Ok(notes
        .into_iter()
        .map(|note| PreparedItem {
            cost_text: format!(
                "{}\n{}\n{}",
                first_line(note.content.as_str()),
                note.source_type,
                note.created_at
            ),
            value: MemoryContext {
                first_line: first_line(note.content.as_str()),
                source_type: note.source_type,
                created_at: note.created_at,
            },
        })
        .collect())
}

fn prepare_recent_changes(
    workspace: &Path,
    record: &SymbolRecord,
) -> Vec<PreparedItem<ChangeContext>> {
    let Some(git) = GitContext::open(workspace) else {
        return Vec::new();
    };
    git.file_log(Path::new(record.file_path.as_str()), 5)
        .into_iter()
        .map(|entry| PreparedItem {
            cost_text: format!("{}\n{}\n{}", entry.message, entry.hash, entry.timestamp),
            value: ChangeContext {
                relative_age: format_relative_age(entry.timestamp),
                message: entry.message,
                short_sha: entry.hash.chars().take(7).collect(),
            },
        })
        .collect()
}

fn prepare_transitive_dependencies(
    store: &SqliteStore,
    record: &SymbolRecord,
    depth: u32,
) -> Result<Vec<PreparedItem<TransitiveDependencyContext>>> {
    let mut prepared = Vec::new();
    let mut seen = HashSet::new();
    let mut frontier = vec![record.id.clone()];

    for current_depth in 1..=depth {
        let mut next_frontier = Vec::new();
        for source_id in frontier {
            let edges = store
                .get_dependencies(source_id.as_str())
                .with_context(|| format!("failed to list dependencies for {}", source_id))?;
            for edge in edges {
                let Some(target) = store
                    .get_symbol_by_qualified_name(edge.target_qualified_name.as_str())
                    .with_context(|| {
                        format!(
                            "failed to resolve transitive dependency '{}'",
                            edge.target_qualified_name
                        )
                    })?
                else {
                    continue;
                };
                if !seen.insert(target.id.clone()) {
                    continue;
                }
                next_frontier.push(target.id.clone());
                if current_depth < 2 {
                    continue;
                }
                let summary = read_intent_summary(store, target.id.as_str())
                    .unwrap_or_else(|| "No SIR recorded.".to_owned());
                prepared.push(PreparedItem {
                    cost_text: format!(
                        "{}\n{}\n{}\n{}",
                        target.qualified_name, target.file_path, summary, current_depth
                    ),
                    value: TransitiveDependencyContext {
                        qualified_name: target.qualified_name,
                        file_path: target.file_path,
                        intent_summary: summary,
                        depth: current_depth,
                    },
                });
            }
        }
        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }

    Ok(prepared)
}

fn read_intent_summary(store: &SqliteStore, symbol_id: &str) -> Option<String> {
    let blob = store.read_sir_blob(symbol_id).ok().flatten()?;
    let sir = serde_json::from_str::<SirAnnotation>(&blob).ok()?;
    let sentence = first_sentence(sir.intent.as_str());
    if sentence.is_empty() {
        None
    } else {
        Some(sentence)
    }
}

fn coupling_entry(file_path: &str, edge: CouplingEdgeRecord) -> PreparedItem<CouplingContext> {
    let other_file = if edge.file_a == file_path {
        edge.file_b
    } else {
        edge.file_a
    };
    PreparedItem {
        cost_text: format!("{other_file}\n{}", edge.fused_score),
        value: CouplingContext {
            file_path: other_file,
            fused_score: edge.fused_score,
        },
    }
}

fn allocate_single_symbol(
    prepared: Vec<PreparedSymbolContext>,
    max_tokens: usize,
) -> Result<LegacyContextDocument> {
    let mut iter = prepared.into_iter();
    let prepared = iter
        .next()
        .ok_or_else(|| anyhow!("no symbol contexts prepared"))?;
    let mut budget = BudgetAllocator::new(max_tokens);
    if !budget.try_add(prepared.base_cost.as_str()) {
        return Err(anyhow!(
            "symbol source + intent exceeds the requested context budget"
        ));
    }

    let mut output = prepared.base_output;
    allocate_tier_list(
        &mut budget,
        &mut output.test_guards,
        prepared.test_guards,
        "test guards",
        &mut output.notices,
    );
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.dependencies,
            prepared.dependencies,
            "dependencies",
            &mut output.notices,
        );
    }
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.callers,
            prepared.callers,
            "callers",
            &mut output.notices,
        );
    }
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.coupling,
            prepared.coupling,
            "coupling entries",
            &mut output.notices,
        );
    }
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.memory,
            prepared.memory,
            "memory notes",
            &mut output.notices,
        );
    }
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.recent_changes,
            prepared.recent_changes,
            "recent changes",
            &mut output.notices,
        );
    }
    if budget.remaining() > 0
        && let Some(health) = prepared.health
        && budget.try_add(health.cost_text.as_str())
    {
        output.health = Some(health.value);
    }
    if budget.remaining() > 0 {
        allocate_tier_list(
            &mut budget,
            &mut output.transitive_dependencies,
            prepared.transitive_dependencies,
            "transitive dependencies",
            &mut output.notices,
        );
    }

    Ok(LegacyContextDocument {
        symbols: vec![output],
        used_tokens: budget.used_tokens,
        max_tokens,
        notices: Vec::new(),
    })
}

fn allocate_batch_symbols(
    prepared: Vec<PreparedSymbolContext>,
    max_tokens: usize,
) -> Result<LegacyContextDocument> {
    if prepared.is_empty() {
        return Err(anyhow!("no symbol contexts prepared"));
    }

    let mut budget = BudgetAllocator::new(max_tokens);
    let mut outputs = Vec::new();
    let mut included = Vec::new();
    let mut notices = Vec::new();

    for (index, prepared_symbol) in prepared.iter().enumerate() {
        if budget.try_add(prepared_symbol.base_cost.as_str()) {
            outputs.push(prepared_symbol.base_output.clone());
            included.push(index);
            continue;
        }

        if outputs.is_empty() {
            return Err(anyhow!(
                "mandatory source + intent for '{}' exceeds the requested context budget",
                prepared_symbol.base_output.qualified_name
            ));
        }

        let omitted = prepared.len().saturating_sub(index);
        notices.push(format!(
            "Context truncated: {omitted} symbol(s) omitted because mandatory source + intent would exceed the remaining budget"
        ));
        break;
    }

    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.test_guards,
                prepared_symbol.test_guards.clone(),
                "test guards",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.dependencies,
                prepared_symbol.dependencies.clone(),
                "dependencies",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.callers,
                prepared_symbol.callers.clone(),
                "callers",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.coupling,
                prepared_symbol.coupling.clone(),
                "coupling entries",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.memory,
                prepared_symbol.memory.clone(),
                "memory notes",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.recent_changes,
                prepared_symbol.recent_changes.clone(),
                "recent changes",
                &mut output.notices,
            );
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
            && let Some(health) = prepared_symbol.health.clone()
            && budget.try_add(health.cost_text.as_str())
        {
            output.health = Some(health.value);
        }
    }
    for (slot, prepared_index) in included.iter().enumerate() {
        if let (Some(prepared_symbol), Some(output)) =
            (prepared.get(*prepared_index), outputs.get_mut(slot))
        {
            allocate_batch_tier_list(
                &mut budget,
                &mut output.transitive_dependencies,
                prepared_symbol.transitive_dependencies.clone(),
                "transitive dependencies",
                &mut output.notices,
            );
        }
    }

    Ok(LegacyContextDocument {
        symbols: outputs,
        used_tokens: budget.used_tokens,
        max_tokens,
        notices,
    })
}

fn allocate_tier_list<T: Clone>(
    budget: &mut BudgetAllocator,
    target: &mut Vec<T>,
    prepared: Vec<PreparedItem<T>>,
    label: &str,
    notices: &mut Vec<String>,
) {
    for (index, item) in prepared.iter().enumerate() {
        if budget.try_add(item.cost_text.as_str()) {
            target.push(item.value.clone());
            continue;
        }

        let omitted = prepared.len().saturating_sub(index);
        if omitted > 0 {
            notices.push(format!(
                "Context truncated: {omitted} {label} omitted to fit budget"
            ));
        }
        break;
    }
}

fn allocate_batch_tier_list<T: Clone>(
    budget: &mut BudgetAllocator,
    target: &mut Vec<T>,
    prepared: Vec<PreparedItem<T>>,
    label: &str,
    notices: &mut Vec<String>,
) {
    if budget.remaining() == 0 {
        return;
    }

    for (index, item) in prepared.iter().enumerate() {
        if budget.try_add(item.cost_text.as_str()) {
            target.push(item.value.clone());
            continue;
        }

        let omitted = prepared.len().saturating_sub(index);
        if omitted > 0 {
            notices.push(format!(
                "Context truncated: {omitted} {label} omitted to fit budget"
            ));
        }
        break;
    }
}

fn render_legacy_document(document: &LegacyContextDocument, format: LegacyOutputFormat) -> String {
    match format {
        LegacyOutputFormat::Markdown => render_legacy_markdown(document),
        LegacyOutputFormat::Json => {
            serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned())
        }
        LegacyOutputFormat::Text => render_legacy_text(document),
    }
}

fn render_legacy_markdown(document: &LegacyContextDocument) -> String {
    let mut out = String::new();
    for (index, symbol) in document.symbols.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("# Symbol: {}\n\n", symbol.qualified_name));
        out.push_str(&format!(
            "**Kind:** {} | **File:** {} | **Staleness:** {}\n\n",
            symbol.kind,
            symbol.file_path,
            symbol
                .staleness_score
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_owned())
        ));
        out.push_str("## Source\n");
        out.push_str(&format!(
            "```{}\n{}\n```\n\n",
            symbol.language, symbol.source_code
        ));
        out.push_str("## Intent\n");
        out.push_str(&format!("{}\n\n", symbol.intent));
        out.push_str("## Behavior\n");
        if symbol.behavior.is_empty() {
            out.push_str("(none)\n\n");
        } else {
            for entry in &symbol.behavior {
                out.push_str(&format!("- {entry}\n"));
            }
            out.push('\n');
        }
        render_markdown_list(
            &mut out,
            "## Test Guards",
            symbol
                .test_guards
                .iter()
                .map(|guard| format!("- `{}` — \"{}\"", guard.test_name, guard.description)),
        );
        render_markdown_list(
            &mut out,
            "## Dependencies (1 hop)",
            symbol.dependencies.iter().map(|dependency| {
                format!(
                    "- `{}` — {}",
                    dependency.qualified_name, dependency.intent_summary
                )
            }),
        );
        render_markdown_list(
            &mut out,
            "## Callers",
            symbol
                .callers
                .iter()
                .map(|caller| format!("- `{}` ({})", caller.qualified_name, caller.file_path)),
        );
        render_markdown_list(
            &mut out,
            "## Coupling",
            symbol
                .coupling
                .iter()
                .map(|entry| format!("- `{}` — fused {:.2}", entry.file_path, entry.fused_score)),
        );
        render_markdown_list(
            &mut out,
            "## Memory",
            symbol.memory.iter().map(|note| {
                format!(
                    "- {} ({}, {})",
                    note.first_line,
                    note.source_type,
                    format_relative_age(note.created_at)
                )
            }),
        );
        render_markdown_list(
            &mut out,
            "## Recent Changes",
            symbol.recent_changes.iter().map(|change| {
                format!(
                    "- {}: {} ({})",
                    change.relative_age, change.message, change.short_sha
                )
            }),
        );
        if let Some(health) = &symbol.health {
            out.push_str("## Health\n");
            out.push_str(&format!(
                "- Staleness: {}\n- Generation: {} / {}\n- Updated: {}\n- Status: {}\n\n",
                health
                    .staleness_score
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "n/a".to_owned()),
                health.generation_pass,
                health.model,
                format_relative_age(health.updated_at),
                health.sir_status
            ));
        }
        render_markdown_list(
            &mut out,
            "## Transitive Dependencies",
            symbol.transitive_dependencies.iter().map(|entry| {
                format!(
                    "- depth {}: `{}` — {}",
                    entry.depth, entry.qualified_name, entry.intent_summary
                )
            }),
        );
        for notice in &symbol.notices {
            out.push_str(&format!("> [{notice}]\n"));
        }
        out.push('\n');
    }
    for notice in &document.notices {
        out.push_str(&format!("> [{notice}]\n"));
    }
    out.push_str(&format!(
        "> [Context budget: {} / {} tokens used]\n",
        document.used_tokens, document.max_tokens
    ));
    out
}

fn render_legacy_text(document: &LegacyContextDocument) -> String {
    let mut out = String::new();
    for (index, symbol) in document.symbols.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("Symbol: {}\n", symbol.qualified_name));
        out.push_str(&format!(
            "Kind: {} | File: {} | Staleness: {}\n\n",
            symbol.kind,
            symbol.file_path,
            symbol
                .staleness_score
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_owned())
        ));
        out.push_str("Source:\n");
        out.push_str(&symbol.source_code);
        out.push_str("\n\nIntent:\n");
        out.push_str(&symbol.intent);
        out.push_str("\n\nBehavior:\n");
        if symbol.behavior.is_empty() {
            out.push_str("(none)\n");
        } else {
            for entry in &symbol.behavior {
                out.push_str(&format!("- {entry}\n"));
            }
        }
        render_text_section(
            &mut out,
            "Test Guards",
            symbol
                .test_guards
                .iter()
                .map(|guard| format!("- {} — {}", guard.test_name, guard.description)),
        );
        render_text_section(
            &mut out,
            "Dependencies",
            symbol.dependencies.iter().map(|dependency| {
                format!(
                    "- {} — {}",
                    dependency.qualified_name, dependency.intent_summary
                )
            }),
        );
        render_text_section(
            &mut out,
            "Callers",
            symbol
                .callers
                .iter()
                .map(|caller| format!("- {} ({})", caller.qualified_name, caller.file_path)),
        );
        render_text_section(
            &mut out,
            "Coupling",
            symbol
                .coupling
                .iter()
                .map(|entry| format!("- {} — fused {:.2}", entry.file_path, entry.fused_score)),
        );
        render_text_section(
            &mut out,
            "Memory",
            symbol
                .memory
                .iter()
                .map(|note| format!("- {} ({})", note.first_line, note.source_type)),
        );
        render_text_section(
            &mut out,
            "Recent Changes",
            symbol.recent_changes.iter().map(|change| {
                format!(
                    "- {}: {} ({})",
                    change.relative_age, change.message, change.short_sha
                )
            }),
        );
        if let Some(health) = &symbol.health {
            out.push_str("Health:\n");
            out.push_str(&format!(
                "- Staleness: {}\n- Generation: {} / {}\n- Updated: {}\n- Status: {}\n",
                health
                    .staleness_score
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "n/a".to_owned()),
                health.generation_pass,
                health.model,
                format_relative_age(health.updated_at),
                health.sir_status
            ));
        }
        render_text_section(
            &mut out,
            "Transitive Dependencies",
            symbol.transitive_dependencies.iter().map(|entry| {
                format!(
                    "- depth {}: {} — {}",
                    entry.depth, entry.qualified_name, entry.intent_summary
                )
            }),
        );
        for notice in &symbol.notices {
            out.push_str(&format!("NOTE: {notice}\n"));
        }
        out.push('\n');
    }
    for notice in &document.notices {
        out.push_str(&format!("NOTE: {notice}\n"));
    }
    out.push_str(&format!(
        "Context budget: {} / {} tokens used\n",
        document.used_tokens, document.max_tokens
    ));
    out
}

fn render_markdown_list<I>(out: &mut String, heading: &str, items: I)
where
    I: IntoIterator<Item = String>,
{
    out.push_str(heading);
    out.push('\n');
    let collected = items.into_iter().collect::<Vec<_>>();
    if collected.is_empty() {
        out.push_str("(none)\n\n");
        return;
    }
    for item in collected {
        out.push_str(&item);
        out.push('\n');
    }
    out.push('\n');
}

fn render_text_section<I>(out: &mut String, heading: &str, items: I)
where
    I: IntoIterator<Item = String>,
{
    out.push_str(heading);
    out.push_str(":\n");
    let collected = items.into_iter().collect::<Vec<_>>();
    if collected.is_empty() {
        out.push_str("(none)\n");
        return;
    }
    for item in collected {
        out.push_str(&item);
        out.push('\n');
    }
}

fn parse_legacy_output_format(raw: &str) -> Result<LegacyOutputFormat> {
    match raw.trim() {
        "markdown" => Ok(LegacyOutputFormat::Markdown),
        "json" => Ok(LegacyOutputFormat::Json),
        "text" => Ok(LegacyOutputFormat::Text),
        other => Err(anyhow!(
            "unsupported output format '{other}', expected one of: markdown, json, text"
        )),
    }
}

fn parse_include_sections(raw: Option<&str>) -> Result<IncludeSections> {
    let Some(raw) = raw else {
        return Ok(IncludeSections::all());
    };

    let mut include = IncludeSections {
        deps: false,
        dependents: false,
        coupling: false,
        tests: false,
        memory: false,
        changes: false,
        health: false,
    };
    for token in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match token {
            "deps" => include.deps = true,
            "dependents" => include.dependents = true,
            "coupling" => include.coupling = true,
            "tests" => include.tests = true,
            "memory" => include.memory = true,
            "changes" => include.changes = true,
            "health" => include.health = true,
            other => {
                return Err(anyhow!(
                    "unsupported include section '{other}', expected any of: deps, dependents, coupling, tests, memory, changes, health"
                ));
            }
        }
    }
    Ok(include)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use aether_core::Language;
    use aether_sir::{FileSir, synthetic_file_sir_id};
    use aether_store::{
        ProjectNoteStore, SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore,
        SymbolRecord, SymbolRelationStore, TestIntentRecord, TestIntentStore,
    };
    use tempfile::tempdir;

    use super::{
        ContextArgs, ExportDocument, LegacyContextDocument, TestGuard, canonicalize_file_sir_json,
        parse_include_sections, render_export_markdown, render_legacy_markdown,
        run_context_command, run_sir_context_command,
    };
    use crate::cli::SirContextArgs;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"

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

    fn write_demo_source(workspace: &Path) -> String {
        let relative = "src/lib.rs";
        fs::create_dir_all(workspace.join("src")).expect("create src");
        fs::write(
            workspace.join(relative),
            "pub fn alpha() -> i32 { 1 }\n\npub fn beta() -> i32 { alpha() }\n\npub fn gamma() -> i32 { beta() }\n",
        )
        .expect("write source");
        relative.to_owned()
    }

    fn parse_symbols(workspace: &Path, relative: &str) -> Vec<aether_core::Symbol> {
        let source = fs::read_to_string(workspace.join(relative)).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(Language::Rust, relative, &source)
            .expect("parse")
    }

    fn symbol_record(symbol: &aether_core::Symbol) -> SymbolRecord {
        SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        }
    }

    fn write_file_rollup(store: &SqliteStore, file_path: &str) {
        let rollup = FileSir {
            intent: "File rollup summary".to_owned(),
            exports: vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()],
            side_effects: vec!["writes cache".to_owned()],
            dependencies: vec!["std".to_owned()],
            error_modes: vec!["io".to_owned()],
            symbol_count: 3,
            confidence: 0.8,
        };
        let canonical = canonicalize_file_sir_json(&rollup);
        let rollup_id = synthetic_file_sir_id("rust", file_path);
        store
            .write_sir_blob(&rollup_id, &canonical)
            .expect("write rollup");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: rollup_id,
                sir_hash: aether_sir::file_sir_hash(&rollup),
                sir_version: 1,
                provider: "rollup".to_owned(),
                model: "deterministic".to_owned(),
                generation_pass: "single".to_owned(),
                prompt_hash: None,
                staleness_score: Some(0.1),
                updated_at: 1_700_000_002,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_002,
            })
            .expect("write rollup meta");
    }

    fn seed_workspace() -> (tempfile::TempDir, SqliteStore, Vec<aether_core::Symbol>) {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_source(temp.path());
        let symbols = parse_symbols(temp.path(), &relative);
        let store = SqliteStore::open(temp.path()).expect("open store");

        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            store
                .write_sir_blob(
                    symbol.id.as_str(),
                    &format!(
                        "{{\"confidence\":0.4,\"dependencies\":[],\"error_modes\":[\"io\"],\"inputs\":[],\"intent\":\"{} intent\",\"outputs\":[],\"side_effects\":[\"writes cache\"]}}",
                        symbol.qualified_name
                    ),
                )
                .expect("write sir");
            store
                .upsert_sir_meta(SirMetaRecord {
                    id: symbol.id.clone(),
                    sir_hash: format!("hash-{}", symbol.id),
                    sir_version: 1,
                    provider: "mock".to_owned(),
                    model: "mock".to_owned(),
                    generation_pass: "scan".to_owned(),
                    prompt_hash: Some("src123|nbr123|cfg123".to_owned()),
                    staleness_score: Some(0.25),
                    updated_at: 1_700_000_001,
                    sir_status: "fresh".to_owned(),
                    last_error: None,
                    last_attempt_at: 1_700_000_001,
                })
                .expect("upsert meta");
        }

        let alpha = symbols.first().expect("alpha");
        let beta = symbols.get(1).expect("beta");
        let gamma = symbols.get(2).expect("gamma");
        store
            .upsert_edges(&[aether_core::SymbolEdge {
                source_id: beta.id.clone(),
                target_qualified_name: alpha.qualified_name.clone(),
                edge_kind: aether_core::EdgeKind::DependsOn,
                file_path: beta.file_path.clone(),
            }])
            .expect("upsert deps");
        store
            .upsert_edges(&[aether_core::SymbolEdge {
                source_id: gamma.id.clone(),
                target_qualified_name: beta.qualified_name.clone(),
                edge_kind: aether_core::EdgeKind::Calls,
                file_path: gamma.file_path.clone(),
            }])
            .expect("upsert callers");
        store
            .replace_test_intents_for_file(
                alpha.file_path.as_str(),
                &[TestIntentRecord {
                    intent_id: "intent-1".to_owned(),
                    file_path: alpha.file_path.clone(),
                    test_name: "test_alpha".to_owned(),
                    intent_text: "guards alpha behavior".to_owned(),
                    group_label: None,
                    language: "rust".to_owned(),
                    symbol_id: Some(alpha.id.clone()),
                    created_at: 1_700_000_000,
                    updated_at: 1_700_000_000,
                }],
            )
            .expect("replace test intents");
        store
            .upsert_project_note(aether_store::ProjectNoteRecord {
                note_id: "note-1".to_owned(),
                content: "Alpha note\nsecond line".to_owned(),
                content_hash: "hash-note-1".to_owned(),
                source_type: "manual".to_owned(),
                source_agent: None,
                tags: Vec::new(),
                entity_refs: Vec::new(),
                file_refs: vec![alpha.file_path.clone()],
                symbol_refs: vec![alpha.id.clone()],
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                access_count: 0,
                last_accessed_at: None,
                is_archived: false,
            })
            .expect("upsert project note");
        write_file_rollup(&store, "src/lib.rs");

        (temp, store, symbols)
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("run git");
        if !output.status.success() {
            panic!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    fn init_git_repo(workspace: &Path) {
        run_git(workspace, &["init"]);
        run_git(workspace, &["branch", "-M", "main"]);
        run_git(workspace, &["config", "user.name", "Aether Test"]);
        run_git(
            workspace,
            &["config", "user.email", "aether-test@example.com"],
        );
    }

    #[test]
    fn include_parser_rejects_unknown_sections() {
        let err = parse_include_sections(Some("deps,wat")).expect_err("expected parse error");
        assert!(err.to_string().contains("unsupported include section"));
    }

    #[test]
    fn context_file_target_prefers_rollup_and_is_markdown() {
        let (temp, _store, _symbols) = seed_workspace();
        let output = temp.path().join("context.md");

        run_context_command(
            temp.path(),
            ContextArgs {
                targets: vec!["src/lib.rs".to_owned()],
                symbol: None,
                file: None,
                overview: false,
                branch: None,
                preset: None,
                format: Some("markdown".to_owned()),
                budget: Some(8_000),
                depth: Some(2),
                include: None,
                exclude: None,
                task: None,
                context_lines: None,
                output: Some(output.display().to_string()),
            },
        )
        .expect("run context");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("## Target File: src/lib.rs"));
        assert!(rendered.contains("### File Rollup"));
        assert!(rendered.contains("File rollup summary"));
        assert!(rendered.contains("## Budget Usage"));
    }

    #[test]
    fn context_unindexed_file_falls_back_to_source_only() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_source(temp.path());
        let output = temp.path().join("context.md");

        run_context_command(
            temp.path(),
            ContextArgs {
                targets: vec![relative],
                symbol: None,
                file: None,
                overview: false,
                branch: None,
                preset: None,
                format: Some("markdown".to_owned()),
                budget: Some(4_000),
                depth: Some(2),
                include: None,
                exclude: Some("graph,coupling,health,drift,memory,tests,sir".to_owned()),
                task: None,
                context_lines: None,
                output: Some(output.display().to_string()),
            },
        )
        .expect("run context");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("index data unavailable for src/lib.rs"));
        assert!(rendered.contains("pub fn alpha() -> i32 { 1 }"));
    }

    #[test]
    fn context_overview_without_index_is_graceful() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let output = temp.path().join("overview.md");

        run_context_command(
            temp.path(),
            ContextArgs {
                targets: Vec::new(),
                symbol: None,
                file: None,
                overview: true,
                branch: None,
                preset: None,
                format: Some("markdown".to_owned()),
                budget: Some(2_000),
                depth: Some(2),
                include: None,
                exclude: None,
                task: None,
                context_lines: None,
                output: Some(output.display().to_string()),
            },
        )
        .expect("run context overview");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("## Project Overview"));
        assert!(rendered.contains("run `aetherd --index-once` first"));
    }

    #[test]
    fn context_json_output_is_valid() {
        let (temp, _store, _symbols) = seed_workspace();
        let output = temp.path().join("context.json");

        run_context_command(
            temp.path(),
            ContextArgs {
                targets: vec!["src/lib.rs".to_owned()],
                symbol: None,
                file: None,
                overview: false,
                branch: None,
                preset: None,
                format: Some("json".to_owned()),
                budget: Some(8_000),
                depth: Some(2),
                include: None,
                exclude: None,
                task: None,
                context_lines: None,
                output: Some(output.display().to_string()),
            },
        )
        .expect("run context json");

        let rendered = fs::read_to_string(output).expect("read output");
        let parsed = serde_json::from_str::<serde_json::Value>(&rendered).expect("parse json");
        assert_eq!(parsed["target_sections"].as_array().map(Vec::len), Some(1));
        assert_eq!(parsed["target_sections"][0]["target_kind"], "file");
    }

    #[test]
    fn context_branch_mode_assembles_ranked_symbols_and_persists_history() {
        let (temp, _store, _symbols) = seed_workspace();
        init_git_repo(temp.path());
        run_git(temp.path(), &["add", "."]);
        run_git(temp.path(), &["commit", "-m", "main"]);
        run_git(temp.path(), &["checkout", "-b", "feature/fix-auth"]);
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn alpha() -> i32 { 2 }\n\npub fn beta() -> i32 { alpha() }\n\npub fn gamma() -> i32 { beta() }\n",
        )
        .expect("update source");
        run_git(temp.path(), &["add", "."]);
        run_git(temp.path(), &["commit", "-m", "feature"]);

        let output = temp.path().join("branch-context.md");
        run_context_command(
            temp.path(),
            ContextArgs {
                targets: Vec::new(),
                symbol: None,
                file: None,
                overview: false,
                branch: Some("feature/fix-auth".to_owned()),
                preset: None,
                format: Some("markdown".to_owned()),
                budget: Some(8_000),
                depth: Some(2),
                include: None,
                exclude: None,
                task: Some("repair alpha flow".to_owned()),
                context_lines: None,
                output: Some(output.display().to_string()),
            },
        )
        .expect("run branch context");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("## Target Symbol:"));
        assert!(rendered.contains("sparse-only"));

        let reopened = SqliteStore::open(temp.path()).expect("reopen store");
        let history = reopened
            .list_recent_task_history(10)
            .expect("list task history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].task_description, "repair alpha flow");
        assert_eq!(history[0].branch_name.as_deref(), Some("feature/fix-auth"));
        assert_eq!(history[0].total_symbols, 3);
    }

    #[test]
    fn sir_context_writes_requested_sections_to_output_file() {
        let (temp, _store, symbols) = seed_workspace();
        let alpha = symbols.first().expect("alpha");
        let output = temp.path().join("context.md");

        run_sir_context_command(
            temp.path(),
            SirContextArgs {
                selector: Some(alpha.id.clone()),
                format: "markdown".to_owned(),
                max_tokens: 4_000,
                depth: 2,
                include: Some("deps,dependents,tests,memory,health".to_owned()),
                output: Some(output.display().to_string()),
                symbols: None,
            },
        )
        .expect("run sir-context");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("# Symbol:"));
        assert!(rendered.contains("## Test Guards"));
        assert!(rendered.contains("## Health"));
    }

    #[test]
    fn sir_context_batch_budget_omits_late_symbols_after_mandatory_tier() {
        let (temp, _store, symbols) = seed_workspace();
        let selectors_path = temp.path().join("symbols.txt");
        let contents = symbols
            .iter()
            .map(|symbol| symbol.id.clone())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&selectors_path, contents).expect("write selectors");
        let output = temp.path().join("context.txt");

        run_sir_context_command(
            temp.path(),
            SirContextArgs {
                selector: None,
                format: "text".to_owned(),
                max_tokens: 35,
                depth: 1,
                include: Some("tests".to_owned()),
                output: Some(output.display().to_string()),
                symbols: Some("symbols.txt".to_owned()),
            },
        )
        .expect("run sir-context");

        let rendered = fs::read_to_string(output).expect("read output");
        assert!(rendered.contains("Context budget:"));
        assert!(rendered.contains("NOTE: Context truncated"));
    }

    #[test]
    fn markdown_renderers_emit_budget_footers() {
        let legacy = render_legacy_markdown(&LegacyContextDocument {
            symbols: vec![super::LegacySymbolContext {
                selector: "sel".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                kind: "function".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                staleness_score: Some(0.2),
                source_code: "pub fn alpha() {}".to_owned(),
                intent: "alpha intent".to_owned(),
                behavior: vec!["writes cache".to_owned()],
                test_guards: vec![TestGuard {
                    test_name: "test_alpha".to_owned(),
                    description: "guards alpha".to_owned(),
                }],
                dependencies: Vec::new(),
                callers: Vec::new(),
                coupling: Vec::new(),
                memory: Vec::new(),
                recent_changes: Vec::new(),
                health: None,
                transitive_dependencies: Vec::new(),
                notices: Vec::new(),
            }],
            used_tokens: 10,
            max_tokens: 20,
            notices: Vec::new(),
        });
        assert!(legacy.contains("Context budget: 10 / 20"));

        let export = render_export_markdown(&ExportDocument {
            generated_at: 1_700_000_000,
            project_overview: Default::default(),
            target_sections: Vec::new(),
            budget_usage: super::BudgetUsage {
                max_tokens: 20,
                used_tokens: 10,
                layers: Vec::new(),
            },
            notices: Vec::new(),
        });
        assert!(export.contains("## Budget Usage"));
    }
}
