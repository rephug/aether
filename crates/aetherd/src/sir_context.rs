use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use aether_core::GitContext;
use aether_sir::SirAnnotation;
use aether_store::{
    CouplingEdgeRecord, CozoGraphStore, ProjectNoteStore, SirStateStore, SqliteStore,
    SymbolRelationStore, TestIntentStore,
};
use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::cli::SirContextArgs;
use crate::sir_agent_support::{
    first_line, first_sentence, format_relative_age, load_fresh_symbol_source, output_path,
    read_selector_file, resolve_symbol,
};

const CHARS_PER_TOKEN: f64 = 3.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
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

#[derive(Debug, Default)]
struct BudgetAllocator {
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

#[derive(Debug, Clone, Serialize)]
struct ContextDocument {
    symbols: Vec<SymbolContext>,
    used_tokens: usize,
    max_tokens: usize,
    notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SymbolContext {
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
struct TestGuard {
    test_name: String,
    description: String,
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
struct CouplingContext {
    file_path: String,
    fused_score: f32,
}

#[derive(Debug, Clone, Serialize)]
struct MemoryContext {
    first_line: String,
    source_type: String,
    created_at: i64,
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
struct PreparedItem<T> {
    value: T,
    cost_text: String,
}

#[derive(Debug, Clone)]
struct PreparedSymbolContext {
    base_output: SymbolContext,
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

pub fn run_sir_context_command(workspace: &Path, args: SirContextArgs) -> Result<()> {
    let format = parse_output_format(args.format.as_str())?;
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
    let rendered = render_document(&document, format);

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
    record: &aether_store::SymbolRecord,
    include: IncludeSections,
    depth: u32,
) -> Result<PreparedSymbolContext> {
    let fresh = load_fresh_symbol_source(workspace, record)?;
    let meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to read SIR metadata for {}", record.id))?;
    let sir = store
        .read_sir_blob(record.id.as_str())
        .with_context(|| format!("failed to read SIR blob for {}", record.id))?
        .map(|blob| serde_json::from_str::<SirAnnotation>(&blob))
        .transpose()
        .with_context(|| format!("failed to parse SIR JSON for {}", record.id))?;

    let (intent, behavior) = match sir.as_ref() {
        Some(sir) => {
            let mut behavior = Vec::new();
            behavior.extend(sir.side_effects.iter().cloned());
            behavior.extend(sir.error_modes.iter().cloned());
            (sir.intent.clone(), behavior)
        }
        None => ("No SIR recorded.".to_owned(), Vec::new()),
    };

    let mut base_output = SymbolContext {
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
    let coupling = if include.coupling {
        let (entries, notice) = prepare_coupling(workspace, record)?;
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
    record: &aether_store::SymbolRecord,
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
    record: &aether_store::SymbolRecord,
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
    record: &aether_store::SymbolRecord,
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

fn prepare_coupling(
    workspace: &Path,
    record: &aether_store::SymbolRecord,
) -> Result<(Vec<PreparedItem<CouplingContext>>, Option<String>)> {
    let graph_store = match CozoGraphStore::open_readonly(workspace) {
        Ok(graph) => graph,
        Err(_) => {
            return Ok((
                Vec::new(),
                Some("coupling data unavailable — daemon may hold SurrealDB lock".to_owned()),
            ));
        }
    };
    let edges = match graph_store.list_co_change_edges_for_file(record.file_path.as_str(), 0.0) {
        Ok(edges) => edges,
        Err(_) => {
            return Ok((
                Vec::new(),
                Some("coupling data unavailable — daemon may hold SurrealDB lock".to_owned()),
            ));
        }
    };
    Ok((
        edges
            .into_iter()
            .map(|edge| coupling_entry(record.file_path.as_str(), edge))
            .collect(),
        None,
    ))
}

fn prepare_memory(
    store: &SqliteStore,
    record: &aether_store::SymbolRecord,
) -> Result<Vec<PreparedItem<MemoryContext>>> {
    let mut notes = store
        .list_project_notes_for_file_ref(record.file_path.as_str(), 10)
        .with_context(|| format!("failed to list project notes for {}", record.file_path))?;
    if notes.len() < 10 {
        let remaining = 10_u32.saturating_sub(notes.len() as u32);
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
    record: &aether_store::SymbolRecord,
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
    record: &aether_store::SymbolRecord,
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
) -> Result<ContextDocument> {
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

    Ok(ContextDocument {
        symbols: vec![output],
        used_tokens: budget.used_tokens,
        max_tokens,
        notices: Vec::new(),
    })
}

fn allocate_batch_symbols(
    prepared: Vec<PreparedSymbolContext>,
    max_tokens: usize,
) -> Result<ContextDocument> {
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

    Ok(ContextDocument {
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

fn render_document(document: &ContextDocument, format: OutputFormat) -> String {
    match format {
        OutputFormat::Markdown => render_markdown(document),
        OutputFormat::Json => {
            serde_json::to_string_pretty(document).unwrap_or_else(|_| "{}".to_owned())
        }
        OutputFormat::Text => render_text(document),
    }
}

fn render_markdown(document: &ContextDocument) -> String {
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

fn render_text(document: &ContextDocument) -> String {
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

fn estimate_tokens(content: &str) -> usize {
    ((content.len() as f64) / CHARS_PER_TOKEN).ceil() as usize
}

fn parse_output_format(raw: &str) -> Result<OutputFormat> {
    match raw.trim() {
        "markdown" => Ok(OutputFormat::Markdown),
        "json" => Ok(OutputFormat::Json),
        "text" => Ok(OutputFormat::Text),
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

    use aether_core::Language;
    use aether_store::{
        ProjectNoteStore, SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore,
        SymbolRecord, SymbolRelationStore, TestIntentRecord, TestIntentStore,
    };
    use tempfile::tempdir;

    use super::{parse_include_sections, render_markdown, run_sir_context_command};
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

        (temp, store, symbols)
    }

    #[test]
    fn include_parser_rejects_unknown_sections() {
        let err = parse_include_sections(Some("deps,wat")).expect_err("expected parse error");
        assert!(err.to_string().contains("unsupported include section"));
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
    fn markdown_renderer_emits_budget_footer() {
        let rendered = render_markdown(&super::ContextDocument {
            symbols: vec![super::SymbolContext {
                selector: "sel".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                kind: "function".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                staleness_score: Some(0.2),
                source_code: "pub fn alpha() {}".to_owned(),
                intent: "alpha intent".to_owned(),
                behavior: vec!["writes cache".to_owned()],
                test_guards: Vec::new(),
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

        assert!(rendered.contains("Context budget: 10 / 20"));
    }
}
