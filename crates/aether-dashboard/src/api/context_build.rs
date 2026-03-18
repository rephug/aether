use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const MAX_TREE_FILES: usize = 500;
const CHARS_PER_TOKEN: f64 = 3.5;
const DEFAULT_BUDGET: usize = 32_000;
const DEFAULT_DEPTH: usize = 2;
const GRAPH_NEIGHBOR_LIMIT: usize = 12;

/// Layer budget suggestions as percentages of total budget.
const LAYER_SUGGESTIONS: &[(&str, usize)] = &[
    ("source", 30),
    ("sir", 15),
    ("graph", 15),
    ("tests", 10),
    ("coupling", 8),
    ("memory", 7),
    ("health", 5),
    ("drift", 5),
];

// ── File tree types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FileTreeNode {
    pub path: String,
    #[serde(rename = "type")]
    pub node_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_sir: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FileTreeNode>,
}

#[derive(Debug, Serialize)]
struct FileTreeResponse {
    tree: Vec<FileTreeNode>,
    total_files: usize,
}

// ── Context build types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ContextBuildRequest {
    pub targets: Vec<String>,
    #[serde(default)]
    pub budget: Option<usize>,
    #[serde(default)]
    pub depth: Option<usize>,
    #[serde(default)]
    pub layers: Option<LayerFlags>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LayerFlags {
    #[serde(default = "default_true")]
    pub sir: Option<bool>,
    #[serde(default = "default_true")]
    pub source: Option<bool>,
    #[serde(default = "default_true")]
    pub graph: Option<bool>,
    #[serde(default)]
    pub coupling: Option<bool>,
    #[serde(default)]
    pub health: Option<bool>,
    #[serde(default)]
    pub drift: Option<bool>,
    #[serde(default)]
    pub memory: Option<bool>,
    #[serde(default)]
    pub tests: Option<bool>,
}

fn default_true() -> Option<bool> {
    Some(true)
}

impl LayerFlags {
    fn is_enabled(&self, name: &str) -> bool {
        match name {
            "sir" => self.sir.unwrap_or(true),
            "source" => self.source.unwrap_or(true),
            "graph" => self.graph.unwrap_or(true),
            "coupling" => self.coupling.unwrap_or(false),
            "health" => self.health.unwrap_or(false),
            "drift" => self.drift.unwrap_or(false),
            "memory" => self.memory.unwrap_or(false),
            "tests" => self.tests.unwrap_or(false),
            _ => false,
        }
    }
}

impl Default for LayerFlags {
    fn default() -> Self {
        Self {
            sir: Some(true),
            source: Some(true),
            graph: Some(true),
            coupling: Some(false),
            health: Some(false),
            drift: Some(false),
            memory: Some(false),
            tests: Some(false),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextBuildResponse {
    pub content: String,
    pub budget_usage: BudgetBreakdown,
    pub token_estimate: usize,
    pub target_count: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct BudgetBreakdown {
    pub total: usize,
    pub used: usize,
    pub by_layer: HashMap<String, usize>,
}

// ── File tree handler ────────────────────────────────────────────────────

pub(crate) async fn file_tree_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || build_file_tree(shared.as_ref())).await {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

fn build_file_tree(shared: &SharedState) -> Result<FileTreeResponse, String> {
    let conn = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no sqlite database found".to_string())?;

    // Get file paths with symbol counts
    let mut stmt = conn
        .prepare(
            "SELECT file_path, COUNT(*) as cnt FROM symbols \
             GROUP BY file_path ORDER BY file_path ASC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;

    let file_rows: Vec<(String, usize)> = stmt
        .query_map(rusqlite::params![MAX_TREE_FILES as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let total_files = file_rows.len();

    // Get files that have SIR data
    let sir_files: std::collections::HashSet<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT s.file_path FROM symbols s \
                 JOIN sir r ON r.id = s.id \
                 WHERE COALESCE(TRIM(r.sir_json), '') <> ''",
            )
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect()
    };

    // Build tree structure
    let tree = assemble_tree(&file_rows, &sir_files);

    Ok(FileTreeResponse { tree, total_files })
}

fn assemble_tree(
    files: &[(String, usize)],
    sir_files: &std::collections::HashSet<String>,
) -> Vec<FileTreeNode> {
    // Collect all directory components and build a nested map
    let mut root_children: Vec<FileTreeNode> = Vec::new();

    // Group files by top-level directory
    let mut dir_map: HashMap<String, Vec<&(String, usize)>> = HashMap::new();
    let mut root_files: Vec<&(String, usize)> = Vec::new();

    for entry in files {
        if let Some(slash_pos) = entry.0.find('/') {
            let top_dir = &entry.0[..slash_pos];
            dir_map.entry(top_dir.to_string()).or_default().push(entry);
        } else {
            root_files.push(entry);
        }
    }

    // Add root-level files
    for (path, count) in &root_files {
        root_children.push(FileTreeNode {
            path: path.clone(),
            node_type: "file",
            symbol_count: Some(*count),
            has_sir: Some(sir_files.contains(path.as_str())),
            children: Vec::new(),
        });
    }

    // Add directories (sorted)
    let mut dir_keys: Vec<String> = dir_map.keys().cloned().collect();
    dir_keys.sort();
    for dir in dir_keys {
        let entries = dir_map.get(&dir).unwrap();
        let children = build_subtree(&dir, entries, sir_files);
        root_children.push(FileTreeNode {
            path: dir,
            node_type: "directory",
            symbol_count: None,
            has_sir: None,
            children,
        });
    }

    root_children
}

fn build_subtree(
    prefix: &str,
    files: &[&(String, usize)],
    sir_files: &std::collections::HashSet<String>,
) -> Vec<FileTreeNode> {
    let mut children: Vec<FileTreeNode> = Vec::new();
    let mut sub_dirs: HashMap<String, Vec<&(String, usize)>> = HashMap::new();
    let mut direct_files: Vec<&(String, usize)> = Vec::new();

    let prefix_with_slash = format!("{prefix}/");

    for entry in files {
        let rest = &entry.0[prefix_with_slash.len()..];
        if let Some(slash_pos) = rest.find('/') {
            let next_dir = format!("{prefix}/{}", &rest[..slash_pos]);
            sub_dirs.entry(next_dir).or_default().push(entry);
        } else {
            direct_files.push(entry);
        }
    }

    // Add subdirectories
    let mut sub_keys: Vec<String> = sub_dirs.keys().cloned().collect();
    sub_keys.sort();
    for sub_dir in sub_keys {
        let sub_entries = sub_dirs.get(&sub_dir).unwrap();
        let sub_children = build_subtree(&sub_dir, sub_entries, sir_files);
        children.push(FileTreeNode {
            path: sub_dir,
            node_type: "directory",
            symbol_count: None,
            has_sir: None,
            children: sub_children,
        });
    }

    // Add direct files
    for (path, count) in &direct_files {
        children.push(FileTreeNode {
            path: path.clone(),
            node_type: "file",
            symbol_count: Some(*count),
            has_sir: Some(sir_files.contains(path.as_str())),
            children: Vec::new(),
        });
    }

    children
}

// ── Context build handler ────────────────────────────────────────────────

pub(crate) async fn context_build_handler(
    State(state): State<Arc<DashboardState>>,
    Json(req): Json<ContextBuildRequest>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    let graph = shared.graph.clone();
    let layers = req.layers.unwrap_or_default();
    let budget = req.budget.unwrap_or(DEFAULT_BUDGET);
    let depth = req.depth.unwrap_or(DEFAULT_DEPTH);
    let format = req.format.unwrap_or_else(|| "markdown".to_string());
    let targets = req.targets.clone();
    let _task = req.task.clone();

    // Run the async store queries + graph queries
    let result = support::run_async_with_timeout(move || {
        build_context(ContextBuildParams {
            shared,
            graph,
            targets,
            budget,
            depth,
            layers,
            format,
        })
    })
    .await;

    match result {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

struct ContextBuildParams {
    shared: Arc<SharedState>,
    graph: Arc<dyn aether_store::GraphStore>,
    targets: Vec<String>,
    budget: usize,
    depth: usize,
    layers: LayerFlags,
    format: String,
}

async fn build_context(params: ContextBuildParams) -> Result<ContextBuildResponse, String> {
    let ContextBuildParams {
        shared,
        graph,
        targets,
        budget,
        depth,
        layers,
        format,
    } = params;
    if targets.is_empty() {
        return Ok(ContextBuildResponse {
            content: String::new(),
            budget_usage: BudgetBreakdown {
                total: budget,
                used: 0,
                by_layer: HashMap::new(),
            },
            token_estimate: 0,
            target_count: 0,
        });
    }

    let mut output = String::new();
    let mut layer_usage: HashMap<String, usize> = HashMap::new();
    let mut total_used: usize = 0;

    // Compute per-layer budget limits
    let layer_budgets: HashMap<&str, usize> = LAYER_SUGGESTIONS
        .iter()
        .map(|(name, pct)| (*name, budget * pct / 100))
        .collect();

    // Header
    let header = format!(
        "# AETHER Context\n\n**Targets:** {}\n**Budget:** {} tokens\n\n---\n\n",
        targets.join(", "),
        budget
    );
    let header_tokens = estimate_tokens(&header);
    total_used += header_tokens;
    output.push_str(&header);

    for target_path in &targets {
        // Query symbols for this file
        let symbols = query_file_symbols(&shared.workspace, target_path);

        // Detect language from file extension
        let lang = detect_language(target_path);

        let section_header = format!("## {target_path}\n\n");
        let sh_tokens = estimate_tokens(&section_header);
        if total_used + sh_tokens > budget {
            break;
        }
        total_used += sh_tokens;
        output.push_str(&section_header);

        // SIR layer
        if layers.is_enabled("sir") && !symbols.is_empty() {
            let sir_budget = layer_budgets.get("sir").copied().unwrap_or(0);
            let sir_used = layer_usage.get("sir").copied().unwrap_or(0);
            let sir_remaining = sir_budget.saturating_sub(sir_used);

            if sir_remaining > 0 {
                let symbol_ids: Vec<String> = symbols.iter().map(|s| s.0.clone()).collect();
                let sir_text = build_sir_section(&shared.store, &symbols, &symbol_ids);
                let sir_tokens = estimate_tokens(&sir_text).min(sir_remaining);
                let truncated = truncate_to_tokens(&sir_text, sir_tokens);
                if !truncated.is_empty() {
                    output.push_str(&truncated);
                    output.push('\n');
                    let actual = estimate_tokens(&truncated);
                    *layer_usage.entry("sir".to_string()).or_insert(0) += actual;
                    total_used += actual;
                }
            }
        }

        // Source layer
        if layers.is_enabled("source") {
            let source_budget = layer_budgets.get("source").copied().unwrap_or(0);
            let source_used = layer_usage.get("source").copied().unwrap_or(0);
            let source_remaining = source_budget.saturating_sub(source_used);

            if source_remaining > 0 {
                let source_text = read_source_file(&shared, target_path, &lang);
                if !source_text.is_empty() {
                    let truncated = truncate_to_tokens(&source_text, source_remaining);
                    if !truncated.is_empty() {
                        let block = format!("### Source\n\n```{lang}\n{truncated}\n```\n\n");
                        let actual = estimate_tokens(&block);
                        if total_used + actual <= budget {
                            output.push_str(&block);
                            *layer_usage.entry("source".to_string()).or_insert(0) += actual;
                            total_used += actual;
                        }
                    }
                }
            }
        }

        // Graph layer
        if layers.is_enabled("graph") && depth > 0 && !symbols.is_empty() {
            let graph_budget = layer_budgets.get("graph").copied().unwrap_or(0);
            let graph_used = layer_usage.get("graph").copied().unwrap_or(0);
            let graph_remaining = graph_budget.saturating_sub(graph_used);

            if graph_remaining > 0 {
                let graph_text = build_graph_section(&graph, &shared.store, &symbols, depth).await;
                if !graph_text.is_empty() {
                    let truncated = truncate_to_tokens(&graph_text, graph_remaining);
                    let actual = estimate_tokens(&truncated);
                    if total_used + actual <= budget {
                        output.push_str(&truncated);
                        output.push('\n');
                        *layer_usage.entry("graph".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        // Coupling layer (best-effort)
        if layers.is_enabled("coupling") {
            let coupling_budget = layer_budgets.get("coupling").copied().unwrap_or(0);
            let coupling_used = layer_usage.get("coupling").copied().unwrap_or(0);
            let coupling_remaining = coupling_budget.saturating_sub(coupling_used);

            if coupling_remaining > 0 {
                let coupling_text = build_coupling_section(&shared, target_path).await;
                if !coupling_text.is_empty() {
                    let truncated = truncate_to_tokens(&coupling_text, coupling_remaining);
                    let actual = estimate_tokens(&truncated);
                    if total_used + actual <= budget {
                        output.push_str(&truncated);
                        output.push('\n');
                        *layer_usage.entry("coupling".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        // Health layer
        if layers.is_enabled("health") {
            let health_budget = layer_budgets.get("health").copied().unwrap_or(0);
            let health_used = layer_usage.get("health").copied().unwrap_or(0);
            let health_remaining = health_budget.saturating_sub(health_used);

            if health_remaining > 0 {
                let health_text = build_health_section(&shared, target_path);
                if !health_text.is_empty() {
                    let actual = estimate_tokens(&health_text);
                    if total_used + actual <= budget {
                        output.push_str(&health_text);
                        output.push('\n');
                        *layer_usage.entry("health".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        // Drift layer
        if layers.is_enabled("drift") {
            let drift_budget = layer_budgets.get("drift").copied().unwrap_or(0);
            let drift_used = layer_usage.get("drift").copied().unwrap_or(0);
            let drift_remaining = drift_budget.saturating_sub(drift_used);

            if drift_remaining > 0 {
                let drift_text = build_drift_section(&shared, target_path);
                if !drift_text.is_empty() {
                    let truncated = truncate_to_tokens(&drift_text, drift_remaining);
                    let actual = estimate_tokens(&truncated);
                    if total_used + actual <= budget {
                        output.push_str(&truncated);
                        output.push('\n');
                        *layer_usage.entry("drift".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        // Tests layer
        if layers.is_enabled("tests") {
            let tests_budget = layer_budgets.get("tests").copied().unwrap_or(0);
            let tests_used = layer_usage.get("tests").copied().unwrap_or(0);
            let tests_remaining = tests_budget.saturating_sub(tests_used);

            if tests_remaining > 0 {
                let tests_text = build_tests_section(&shared, target_path);
                if !tests_text.is_empty() {
                    let truncated = truncate_to_tokens(&tests_text, tests_remaining);
                    let actual = estimate_tokens(&truncated);
                    if total_used + actual <= budget {
                        output.push_str(&truncated);
                        output.push('\n');
                        *layer_usage.entry("tests".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        // Memory layer
        if layers.is_enabled("memory") {
            let memory_budget = layer_budgets.get("memory").copied().unwrap_or(0);
            let memory_used = layer_usage.get("memory").copied().unwrap_or(0);
            let memory_remaining = memory_budget.saturating_sub(memory_used);

            if memory_remaining > 0 {
                let memory_text = build_memory_section(&shared, target_path);
                if !memory_text.is_empty() {
                    let truncated = truncate_to_tokens(&memory_text, memory_remaining);
                    let actual = estimate_tokens(&truncated);
                    if total_used + actual <= budget {
                        output.push_str(&truncated);
                        output.push('\n');
                        *layer_usage.entry("memory".to_string()).or_insert(0) += actual;
                        total_used += actual;
                    }
                }
            }
        }

        output.push_str("---\n\n");
    }

    // Budget summary footer
    let footer = format_budget_footer(budget, total_used, &layer_usage);
    output.push_str(&footer);

    // Format conversion (XML/compact wrapping)
    let final_content = match format.as_str() {
        "xml" => wrap_as_xml(&output, budget, total_used, &layer_usage),
        "compact" => wrap_as_compact(&output, budget, total_used, &targets),
        _ => output,
    };

    let token_estimate = estimate_tokens(&final_content);

    Ok(ContextBuildResponse {
        content: final_content,
        budget_usage: BudgetBreakdown {
            total: budget,
            used: total_used,
            by_layer: layer_usage,
        },
        token_estimate,
        target_count: targets.len(),
    })
}

// ── Layer builders ───────────────────────────────────────────────────────

/// Returns (symbol_id, qualified_name, kind) tuples for all symbols in a file.
fn query_file_symbols(
    workspace: &std::path::Path,
    file_path: &str,
) -> Vec<(String, String, String)> {
    let conn = match support::open_meta_sqlite_ro(workspace) {
        Ok(Some(c)) => c,
        _ => return Vec::new(),
    };
    let mut stmt = match conn.prepare(
        "SELECT id, qualified_name, kind FROM symbols WHERE file_path = ?1 ORDER BY qualified_name ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    stmt.query_map(rusqlite::params![file_path], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

fn build_sir_section(
    store: &Arc<aether_store::SqliteStore>,
    symbols: &[(String, String, String)],
    symbol_ids: &[String],
) -> String {
    let sir_blobs = match store.list_sir_blobs_for_ids(symbol_ids) {
        Ok(blobs) => blobs,
        Err(_) => return String::new(),
    };

    if sir_blobs.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Semantic Intelligence (SIR)\n\n");

    for (id, qname, kind) in symbols {
        if let Some(blob) = sir_blobs.get(id)
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(blob)
        {
            let intent = parsed
                .get("intent")
                .and_then(|v| v.as_str())
                .unwrap_or("(no intent)");
            out.push_str(&format!("**{qname}** ({kind})\n"));
            out.push_str(&format!("- Intent: {intent}\n"));

            if let Some(side_effects) = parsed.get("side_effects").and_then(|v| v.as_array()) {
                for effect in side_effects.iter().filter_map(|v| v.as_str()) {
                    out.push_str(&format!("- Side effect: {effect}\n"));
                }
            }
            if let Some(error_modes) = parsed.get("error_modes").and_then(|v| v.as_array()) {
                for mode in error_modes.iter().filter_map(|v| v.as_str()) {
                    out.push_str(&format!("- Error mode: {mode}\n"));
                }
            }
            out.push('\n');
        }
    }

    out
}

fn read_source_file(shared: &SharedState, file_path: &str, _lang: &str) -> String {
    let full_path = shared.workspace.join(file_path);
    match std::fs::read_to_string(&full_path) {
        Ok(content) => content,
        Err(_) => {
            // File may have been deleted since indexing
            format!("// Source file not found: {file_path}\n")
        }
    }
}

async fn build_graph_section(
    graph: &Arc<dyn aether_store::GraphStore>,
    store: &Arc<aether_store::SqliteStore>,
    symbols: &[(String, String, String)],
    _depth: usize,
) -> String {
    let mut out = String::from("### Dependencies & Callers\n\n");
    let mut has_content = false;

    for (id, qname, _kind) in symbols.iter().take(5) {
        // Get dependencies
        let deps = graph.get_dependencies(id).await.unwrap_or_default();
        let callers = graph.get_callers(qname).await.unwrap_or_default();

        if deps.is_empty() && callers.is_empty() {
            continue;
        }
        has_content = true;

        out.push_str(&format!("**{qname}**\n"));

        if !deps.is_empty() {
            out.push_str("- Dependencies:\n");
            for dep in deps.iter().take(GRAPH_NEIGHBOR_LIMIT) {
                let intent = sir_intent_summary(store, &dep.id);
                out.push_str(&format!(
                    "  - `{}` ({}) — {}\n",
                    dep.qualified_name, dep.file_path, intent
                ));
            }
            if deps.len() > GRAPH_NEIGHBOR_LIMIT {
                out.push_str(&format!(
                    "  - ... and {} more\n",
                    deps.len() - GRAPH_NEIGHBOR_LIMIT
                ));
            }
        }

        if !callers.is_empty() {
            out.push_str("- Callers:\n");
            for caller in callers.iter().take(GRAPH_NEIGHBOR_LIMIT) {
                let intent = sir_intent_summary(store, &caller.id);
                out.push_str(&format!(
                    "  - `{}` ({}) — {}\n",
                    caller.qualified_name, caller.file_path, intent
                ));
            }
            if callers.len() > GRAPH_NEIGHBOR_LIMIT {
                out.push_str(&format!(
                    "  - ... and {} more\n",
                    callers.len() - GRAPH_NEIGHBOR_LIMIT
                ));
            }
        }

        out.push('\n');
    }

    if !has_content {
        return String::new();
    }
    out
}

fn sir_intent_summary(store: &Arc<aether_store::SqliteStore>, symbol_id: &str) -> String {
    let blobs = store
        .list_sir_blobs_for_ids(&[symbol_id.to_string()])
        .unwrap_or_default();
    if let Some(blob) = blobs.get(symbol_id)
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(blob)
        && let Some(intent) = parsed.get("intent").and_then(|v| v.as_str())
    {
        let first_line = intent.lines().next().unwrap_or(intent);
        if first_line.len() > 80 {
            return format!("{}...", &first_line[..77]);
        }
        return first_line.to_string();
    }
    "(no SIR)".to_string()
}

async fn build_coupling_section(shared: &SharedState, file_path: &str) -> String {
    // Best-effort: try to query co-change edges from the surreal graph store
    let surreal = match shared.surreal_graph_store().await {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let edges = match surreal.list_co_change_edges_for_file(file_path, 0.1).await {
        Ok(e) => e,
        Err(_) => return String::new(),
    };

    if edges.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Co-change Coupling\n\n");
    for edge in edges.iter().take(10) {
        let other = if edge.file_a == file_path {
            &edge.file_b
        } else {
            &edge.file_a
        };
        out.push_str(&format!(
            "- `{other}` — fused score: {:.2}\n",
            edge.fused_score
        ));
    }
    out.push('\n');
    out
}

fn build_health_section(shared: &SharedState, _file_path: &str) -> String {
    // Health data comes from cached health score report
    let guard = shared.caches.health_score_report.read().ok();
    let report = guard.as_ref().and_then(|g| g.as_ref());

    if let Some((_instant, report)) = report {
        let mut out = String::from("### Health\n\n");
        out.push_str(&format!(
            "- Workspace score: {}/100\n",
            report.workspace_score
        ));
        out.push('\n');
        out
    } else {
        String::new()
    }
}

fn build_drift_section(shared: &SharedState, file_path: &str) -> String {
    // Query drift findings for file from the analysis tables
    let conn = match support::open_meta_sqlite_ro(shared.workspace.as_path()) {
        Ok(Some(c)) => c,
        _ => return String::new(),
    };

    let mut stmt = match conn.prepare(
        "SELECT symbol_name, drift_type, magnitude, summary FROM drift_findings \
         WHERE file_path = ?1 ORDER BY magnitude DESC LIMIT 10",
    ) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let findings: Vec<(String, String, f64, String)> = stmt
        .query_map(rusqlite::params![file_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2).unwrap_or(0.0),
                row.get::<_, String>(3).unwrap_or_default(),
            ))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if findings.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Drift Findings\n\n");
    for (sym, dtype, mag, summary) in &findings {
        out.push_str(&format!(
            "- **{sym}** [{dtype}] magnitude {mag:.2}: {summary}\n"
        ));
    }
    out.push('\n');
    out
}

fn build_tests_section(shared: &SharedState, file_path: &str) -> String {
    let conn = match support::open_meta_sqlite_ro(shared.workspace.as_path()) {
        Ok(Some(c)) => c,
        _ => return String::new(),
    };

    let mut stmt = match conn.prepare(
        "SELECT test_name, description FROM test_intents \
         WHERE target_file = ?1 ORDER BY test_name ASC LIMIT 20",
    ) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let tests: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![file_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1).unwrap_or_default(),
            ))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if tests.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Test Coverage\n\n");
    for (name, desc) in &tests {
        out.push_str(&format!("- `{name}`: {desc}\n"));
    }
    out.push('\n');
    out
}

fn build_memory_section(shared: &SharedState, file_path: &str) -> String {
    let conn = match support::open_meta_sqlite_ro(shared.workspace.as_path()) {
        Ok(Some(c)) => c,
        _ => return String::new(),
    };

    // Project notes that reference this file
    let mut stmt = match conn.prepare(
        "SELECT content, source_type FROM project_notes \
         WHERE content LIKE ?1 ORDER BY created_at DESC LIMIT 5",
    ) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };

    let pattern = format!("%{file_path}%");
    let notes: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![pattern], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1).unwrap_or_default(),
            ))
        })
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    if notes.is_empty() {
        return String::new();
    }

    let mut out = String::from("### Project Notes\n\n");
    for (content, stype) in &notes {
        let first_line = content.lines().next().unwrap_or(content);
        out.push_str(&format!("- [{stype}] {first_line}\n"));
    }
    out.push('\n');
    out
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn estimate_tokens(content: &str) -> usize {
    ((content.len() as f64) / CHARS_PER_TOKEN).ceil() as usize
}

fn truncate_to_tokens(content: &str, max_tokens: usize) -> String {
    let max_chars = (max_tokens as f64 * CHARS_PER_TOKEN) as usize;
    if content.len() <= max_chars {
        content.to_string()
    } else {
        // Truncate at a line boundary
        let truncated = &content[..max_chars.min(content.len())];
        match truncated.rfind('\n') {
            Some(pos) => format!("{}\n... (truncated)", &truncated[..pos]),
            None => format!("{truncated}\n... (truncated)"),
        }
    }
}

fn detect_language(file_path: &str) -> String {
    if let Some(ext) = file_path.rsplit('.').next() {
        match ext {
            "rs" => "rust",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            "py" => "python",
            "go" => "go",
            "java" => "java",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "json" => "json",
            "md" => "markdown",
            "html" => "html",
            "css" => "css",
            "sql" => "sql",
            _ => ext,
        }
        .to_string()
    } else {
        String::new()
    }
}

fn format_budget_footer(
    budget: usize,
    used: usize,
    layer_usage: &HashMap<String, usize>,
) -> String {
    let mut out = String::from("---\n\n**Budget:** ");
    out.push_str(&format_token_count(used));
    out.push_str(" / ");
    out.push_str(&format_token_count(budget));

    if !layer_usage.is_empty() {
        out.push_str(" | ");
        let mut entries: Vec<(&String, &usize)> = layer_usage.iter().collect();
        entries.sort_by_key(|(name, _)| name.to_string());
        let parts: Vec<String> = entries
            .iter()
            .map(|(name, tokens)| format!("{}: {}", name, format_token_count(**tokens)))
            .collect();
        out.push_str(&parts.join(" | "));
    }

    out.push('\n');
    out
}

fn format_token_count(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}K", n as f64 / 1000.0)
    } else {
        format!("{n}")
    }
}

fn wrap_as_xml(
    content: &str,
    budget: usize,
    used: usize,
    layer_usage: &HashMap<String, usize>,
) -> String {
    let mut out = String::from("<aether_context>\n");
    out.push_str(&format!("  <budget total=\"{budget}\" used=\"{used}\">\n"));
    for (name, tokens) in layer_usage {
        out.push_str(&format!(
            "    <layer name=\"{name}\" tokens=\"{tokens}\" />\n"
        ));
    }
    out.push_str("  </budget>\n");
    out.push_str("  <content><![CDATA[\n");
    out.push_str(content);
    out.push_str("  ]]></content>\n");
    out.push_str("</aether_context>\n");
    out
}

fn wrap_as_compact(content: &str, budget: usize, used: usize, targets: &[String]) -> String {
    let mut out = format!(
        "=== AETHER Context | Budget: {}/{} | Files: {} ===\n",
        format_token_count(used),
        format_token_count(budget),
        targets.len()
    );
    out.push_str(content);
    out
}
