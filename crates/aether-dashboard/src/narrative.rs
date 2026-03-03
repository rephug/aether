use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub icon: String,
    pub description: String,
}

impl Layer {
    pub fn new(name: &str, icon: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            icon: icon.to_owned(),
            description: description.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileInfo {
    pub path: String,
    pub symbol_count: usize,
    pub summary: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolInfo {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub sir_intent: String,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
    pub is_async: bool,
    pub layer: String,
    pub dependents_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SirIntent {
    pub symbol: String,
    pub intent: String,
    pub side_effects: Vec<String>,
    pub dependencies: Vec<String>,
    pub error_modes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Dep {
    pub name: String,
    pub category: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Dependent {
    pub name: String,
    pub layer: String,
}

#[derive(Debug, Clone, Default)]
pub struct Dependency {
    pub name: String,
    pub reason: Option<String>,
}

pub type LayerMap = HashMap<String, String>;

#[derive(Debug, Clone, Default)]
pub struct LayerAssignmentsCache {
    pub symbol_to_layer: HashMap<String, String>,
    pub symbol_name_to_layer: HashMap<String, String>,
}

pub fn layer_catalog() -> Vec<Layer> {
    vec![
        Layer::new(
            "Interface",
            "🌐",
            "Accepts external input from network, CLI, or HTTP",
        ),
        Layer::new(
            "Core Logic",
            "⚙️",
            "Coordinates commands, orchestration, and business behavior",
        ),
        Layer::new(
            "Data",
            "💾",
            "Manages state and persistence for the project",
        ),
        Layer::new(
            "Wire Format",
            "📦",
            "Parses and serializes frames, codecs, and protocol payloads",
        ),
        Layer::new(
            "Connectors",
            "🔌",
            "Integrates with external systems and remote services",
        ),
        Layer::new(
            "Tests",
            "🧪",
            "Validates behavior with tests and assertions",
        ),
        Layer::new(
            "Utilities",
            "🔧",
            "Provides shared helpers used across layers",
        ),
    ]
}

pub fn layer_by_name(name: &str) -> Layer {
    for layer in layer_catalog() {
        if layer.name.eq_ignore_ascii_case(name) {
            return layer;
        }
    }
    Layer::new(name, "⚙️", "General-purpose implementation layer")
}

pub fn classify_layer(file_path: &str, qualified_name: &str, sir_intent: Option<&str>) -> Layer {
    let file = file_path.to_ascii_lowercase();
    let symbol = qualified_name.to_ascii_lowercase();
    let intent = sir_intent.unwrap_or_default().to_ascii_lowercase();

    if file.contains("/tests/")
        || file.starts_with("tests/")
        || file.ends_with("_test.rs")
        || symbol.contains("::tests")
        || symbol.contains("test::")
    {
        return layer_by_name("Tests");
    }

    if file.contains("/bin/")
        || file.ends_with("/main.rs")
        || file == "main.rs"
        || file.contains("cli")
        || symbol.contains("::main")
        || symbol.contains("::cli")
    {
        return layer_by_name("Interface");
    }

    if file.contains("server")
        || file.contains("handler")
        || file.contains("route")
        || file.contains("api")
        || symbol.contains("server")
        || symbol.contains("handler")
        || symbol.contains("route")
    {
        return layer_by_name("Interface");
    }

    if file.contains("client")
        || file.contains("connector")
        || file.contains("provider")
        || symbol.contains("client")
        || symbol.contains("connector")
        || symbol.contains("provider")
    {
        return layer_by_name("Connectors");
    }

    if file.contains("db")
        || file.contains("store")
        || file.contains("repo")
        || file.contains("cache")
        || file.contains("state")
        || symbol.contains("db")
        || symbol.contains("store")
        || symbol.contains("repo")
        || symbol.contains("cache")
    {
        return layer_by_name("Data");
    }

    if file.contains("frame")
        || file.contains("parse")
        || file.contains("codec")
        || file.contains("wire")
        || file.contains("proto")
        || symbol.contains("frame")
        || symbol.contains("parse")
        || symbol.contains("codec")
        || symbol.contains("wire")
        || symbol.contains("proto")
    {
        return layer_by_name("Wire Format");
    }

    if file.contains("cmd")
        || file.contains("command")
        || file.contains("service")
        || symbol.contains("cmd")
        || symbol.contains("command")
        || symbol.contains("service")
    {
        return layer_by_name("Core Logic");
    }

    if intent.contains("utility") || intent.contains("helper") || symbol.contains("helper") {
        return layer_by_name("Utilities");
    }

    layer_by_name("Core Logic")
}

pub fn compose_project_summary(sir_intents: &[SirIntent], lang: &str, deps: &[Dep]) -> String {
    if sir_intents.is_empty() {
        let stack = if deps.is_empty() {
            "a focused set of local modules".to_owned()
        } else {
            format!("{} core dependencies", deps.len())
        };
        return format!(
            "This {lang} project has not produced full SIR coverage yet. It already shows a coherent structure with {stack}. As more symbols are analyzed, this summary will become more specific about behavior and risk."
        );
    }

    let component_count = sir_intents.len();
    let mut side_effect_total = 0usize;
    let mut dependency_total = 0usize;
    let mut error_total = 0usize;
    let mut intent_terms = BTreeMap::<String, usize>::new();

    for sir in sir_intents {
        side_effect_total += sir.side_effects.len();
        dependency_total += sir.dependencies.len();
        error_total += sir.error_modes.len();

        let normalized = first_keyword(sir.intent.as_str());
        if !normalized.is_empty() {
            *intent_terms.entry(normalized).or_insert(0) += 1;
        }
    }

    let mut top_terms = intent_terms.into_iter().collect::<Vec<_>>();
    top_terms.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));

    let focus_terms = top_terms
        .iter()
        .take(3)
        .map(|(term, _)| term.clone())
        .collect::<Vec<_>>();

    let dependency_sentence = if deps.is_empty() {
        "Its dependency surface is intentionally compact.".to_owned()
    } else {
        let dep_names = deps
            .iter()
            .map(|dep| dep.name.as_str())
            .take(4)
            .collect::<Vec<_>>();
        format!(
            "The runtime stack is anchored by {}.",
            join_human_list(dep_names)
        )
    };

    let risk_sentence = if error_total == 0 && side_effect_total == 0 {
        "Most analyzed components are pure and have low operational risk.".to_owned()
    } else if error_total > side_effect_total {
        format!(
            "Failure handling is explicit, with {} documented error modes across the analyzed symbols.",
            error_total
        )
    } else {
        format!(
            "Operational behavior is visible, with {} side effects and {} documented failure modes.",
            side_effect_total, error_total
        )
    };

    let focus_sentence = if focus_terms.is_empty() {
        "The analyzed intents emphasize practical implementation work.".to_owned()
    } else {
        format!(
            "Its strongest intent themes are {}.",
            join_human_list(focus_terms.iter().map(String::as_str).collect())
        )
    };

    format!(
        "This {lang} project has {component_count} analyzed components with structured intent data. {focus_sentence} {dependency_sentence} {risk_sentence} It also records {dependency_total} explicit internal dependency notes in SIR."
    )
}

pub fn compose_layer_narrative(
    layer: &Layer,
    files: &[FileInfo],
    symbols: &[SymbolInfo],
) -> String {
    let file_count = files.len();
    let symbol_count = symbols.len();
    let first_file = file_summary_at(files, 0);
    let second_file = file_summary_at(files, 1);

    match layer.name.as_str() {
        "Interface" => format!(
            "The Interface layer contains {file_count} files with {symbol_count} components that handle how the project communicates with the outside world. {first_file} {second_file} All interface components ultimately connect to the {} layer for processing.",
            most_depended_layer_from_symbols(symbols)
        ),
        "Core Logic" => {
            let command_names = symbols
                .iter()
                .filter(|symbol| {
                    let lower = symbol.name.to_ascii_lowercase();
                    lower.contains("cmd") || lower.contains("command")
                })
                .map(|symbol| symbol.name.as_str())
                .take(5)
                .collect::<Vec<_>>();

            let command_sentence = if command_names.is_empty() {
                String::new()
            } else {
                format!(
                    "It follows a command pattern where each command ({}) processes a specific operation. ",
                    join_human_list(command_names)
                )
            };

            let relationship_to_data_layer = if has_data_touchpoints(symbols) {
                "Core logic components coordinate closely with the Data layer to read and mutate project state."
            } else {
                "Core logic components are mostly self-contained and expose behavior upward to interface handlers."
            };

            format!(
                "The Core Logic layer is the heart of the project with {symbol_count} components across {file_count} files. {command_sentence}{first_file} {relationship_to_data_layer}"
            )
        }
        "Data" => {
            let top_symbol_narrative = top_symbol_narrative(symbols);
            let side_effects_summary = summarize_side_effects(symbols);
            format!(
                "The Data layer manages the project's state through {symbol_count} components in {file_count} files. {top_symbol_narrative} {side_effects_summary}"
            )
        }
        "Wire Format" => format!(
            "The Wire Format layer handles data serialization and parsing with {symbol_count} components. {first_file} These components are used by both the Interface layer (for incoming data) and the Connectors layer (for outgoing data)."
        ),
        "Connectors" => {
            let file_summaries = top_file_summaries(files, 2);
            format!(
                "The Connectors layer provides {symbol_count} components for communicating with external systems. {file_summaries}"
            )
        }
        "Tests" => format!(
            "The test suite contains {symbol_count} test components across {file_count} files. {}",
            coverage_narrative(symbols)
        ),
        "Utilities" => {
            let file_summaries = top_file_summaries(files, 3);
            format!(
                "The project includes {symbol_count} utility components for common operations. {file_summaries}"
            )
        }
        _ => {
            let summaries = top_file_summaries(files, 3);
            format!(
                "This layer contains {symbol_count} components across {file_count} files. {summaries}"
            )
        }
    }
}

pub fn compose_file_summary(file: &str, symbols: &[SymbolInfo]) -> String {
    if symbols.is_empty() {
        return format!("{file} currently has no indexed components.");
    }

    let symbol_count = symbols.len();
    let kind_focus = dominant_kind(symbols);
    let lead_intent = symbols
        .iter()
        .map(|symbol| first_sentence(symbol.sir_intent.as_str()))
        .find(|sentence| !sentence.is_empty())
        .unwrap_or_else(|| "It coordinates key project behavior".to_owned());

    if symbol_count == 1 {
        return format!("{file} defines one {kind_focus} component. {lead_intent}");
    }

    let names = symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .take(3)
        .collect::<Vec<_>>();

    format!(
        "{file} groups {symbol_count} {kind_focus} components, including {}. {lead_intent}",
        join_human_list(names)
    )
}

pub fn compose_dependents_narrative(
    name: &str,
    dependents: &[Dependent],
    layers: &LayerMap,
) -> String {
    if dependents.is_empty() {
        return format!("Nothing else in the project directly uses {name}.");
    }

    if dependents.len() <= 3 {
        let names = dependents
            .iter()
            .map(|dependent| dependent.name.as_str())
            .collect::<Vec<_>>();
        return format!("{} depend on {name}.", join_human_list(names));
    }

    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for dependent in dependents {
        let layer_name = if dependent.layer.trim().is_empty() {
            layers
                .get(dependent.name.as_str())
                .cloned()
                .unwrap_or_else(|| "Core Logic".to_owned())
        } else {
            dependent.layer.clone()
        };
        grouped
            .entry(layer_name)
            .or_default()
            .push(dependent.name.clone());
    }

    let grouped_phrase = grouped
        .into_iter()
        .map(|(layer, names)| {
            if names.len() == 1 {
                format!("the {} component in the {layer} layer", names[0])
            } else {
                format!(
                    "all {} components in the {layer} layer ({})",
                    names.len(),
                    join_human_list(names.iter().map(String::as_str).collect())
                )
            }
        })
        .collect::<Vec<_>>();

    format!(
        "{name} is central to the project - {} components depend on it, including {}.",
        dependents.len(),
        join_human_list(grouped_phrase.iter().map(String::as_str).collect())
    )
}

pub fn compose_dependencies_narrative(name: &str, deps: &[Dependency]) -> String {
    if deps.is_empty() {
        return format!("{name} does not directly depend on other project components.");
    }

    if deps.len() <= 2 {
        let phrases = deps.iter().map(format_dependency).collect::<Vec<_>>();
        return format!(
            "{name} depends on {}.",
            join_human_list(phrases.iter().map(String::as_str).collect())
        );
    }

    let highlighted = deps
        .iter()
        .take(4)
        .map(format_dependency)
        .collect::<Vec<_>>();

    format!(
        "{name} relies on {} direct dependencies, including {}.",
        deps.len(),
        join_human_list(highlighted.iter().map(String::as_str).collect())
    )
}

pub fn qualify_coupling(score: f64) -> &'static str {
    match score {
        s if s < 0.3 => "Weak",
        s if s < 0.6 => "Moderate",
        s if s < 0.8 => "Strong",
        _ => "Very Strong",
    }
}

pub fn qualify_difficulty(
    error_count: usize,
    side_effect_count: usize,
    dep_count: usize,
    is_async: bool,
) -> (&'static str, &'static str) {
    let score = difficulty_score_value("", error_count, side_effect_count, dep_count, is_async);
    if score <= 15.0 {
        ("🟢", "Easy")
    } else if score <= 40.0 {
        ("🟡", "Moderate")
    } else if score <= 65.0 {
        ("🔴", "Hard")
    } else {
        ("⛔", "Very Hard")
    }
}

#[derive(Debug, Clone, Default)]
pub struct DifficultyScore {
    pub score: f64,
    pub emoji: String,
    pub label: String,
    pub guidance: String,
    pub reasons: Vec<String>,
}

pub fn compute_difficulty(symbol: &SymbolInfo) -> DifficultyScore {
    compute_difficulty_from_fields(
        symbol.sir_intent.as_str(),
        symbol.error_modes.len(),
        symbol.side_effects.len(),
        symbol.dependencies.len(),
        symbol.is_async,
    )
}

pub fn compute_difficulty_from_fields(
    intent: &str,
    error_count: usize,
    side_effect_count: usize,
    dep_count: usize,
    is_async: bool,
) -> DifficultyScore {
    let mut reasons = Vec::<String>::new();
    let score = difficulty_score_value(intent, error_count, side_effect_count, dep_count, is_async);

    if error_count > 0 && error_count <= 2 {
        reasons.push(format!("{error_count} failure modes to handle"));
    } else if error_count > 2 {
        reasons.push(format!(
            "{error_count} failure modes — LLMs often miss edge cases"
        ));
    }

    if side_effect_count > 0 && side_effect_count <= 1 {
        reasons.push("has side effects that must be handled correctly".to_owned());
    } else if side_effect_count > 1 {
        reasons.push(format!(
            "{side_effect_count} side effects — LLMs frequently miss cleanup and notifications"
        ));
    }

    if dep_count > 2 && dep_count <= 5 {
        reasons.push(format!(
            "{dep_count} dependencies require context in the prompt"
        ));
    } else if dep_count > 5 {
        reasons.push(format!(
            "{dep_count} dependencies — large context window needed"
        ));
    }

    if has_async_or_concurrent_signals(intent, is_async) {
        reasons
            .push("async/concurrent patterns — LLMs struggle with races and lifetimes".to_owned());
    }

    let (emoji, label, guidance) = if score <= 15.0 {
        (
            "🟢",
            "Easy",
            "Minimal prompting needed. A brief description usually produces correct code.",
        )
    } else if score <= 40.0 {
        (
            "🟡",
            "Moderate",
            "Provide context and verify output. Include type signatures and key constraints.",
        )
    } else if score <= 65.0 {
        (
            "🔴",
            "Hard",
            "Decompose into steps and specify edge cases explicitly. Verify each step before proceeding.",
        )
    } else {
        (
            "⛔",
            "Very Hard",
            "Break into small pieces, specify control flow, enumerate all failure modes. Manual review essential.",
        )
    };

    DifficultyScore {
        score,
        emoji: emoji.to_owned(),
        label: label.to_owned(),
        guidance: guidance.to_owned(),
        reasons,
    }
}

fn difficulty_score_value(
    intent: &str,
    error_count: usize,
    side_effect_count: usize,
    dep_count: usize,
    is_async: bool,
) -> f64 {
    let mut score = 0.0f64;

    if error_count > 0 && error_count <= 2 {
        score += 10.0;
    } else if error_count > 2 {
        score += 30.0;
    }

    if side_effect_count > 0 && side_effect_count <= 1 {
        score += 10.0;
    } else if side_effect_count > 1 {
        score += 25.0;
    }

    if dep_count > 2 && dep_count <= 5 {
        score += 10.0;
    } else if dep_count > 5 {
        score += 20.0;
    }

    if has_async_or_concurrent_signals(intent, is_async) {
        score += 25.0;
    }

    score
}

fn has_async_or_concurrent_signals(intent: &str, is_async: bool) -> bool {
    if is_async {
        return true;
    }

    let lower = intent.to_ascii_lowercase();
    [
        "async",
        "concurrent",
        "spawn",
        "lock",
        "channel",
        "select!",
        "mutex",
        "arc",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_data_touchpoints(symbols: &[SymbolInfo]) -> bool {
    symbols.iter().any(|symbol| {
        symbol
            .dependencies
            .iter()
            .any(|dep| matches_keyword(dep, &["db", "store", "repo", "cache", "state"]))
    })
}

fn summarize_side_effects(symbols: &[SymbolInfo]) -> String {
    let count = symbols
        .iter()
        .map(|symbol| symbol.side_effects.len())
        .sum::<usize>();

    match count {
        0 => {
            "These components are mostly pure and emphasize deterministic state access.".to_owned()
        }
        1..=3 => {
            format!(
                "This layer has {count} documented side effects, mostly tied to state mutation."
            )
        }
        _ => format!(
            "This layer has {count} documented side effects, reflecting heavy coordination with shared state and background behavior."
        ),
    }
}

fn coverage_narrative(symbols: &[SymbolInfo]) -> String {
    let files = symbols
        .iter()
        .map(|symbol| symbol.file_path.as_str())
        .collect::<Vec<_>>();
    let unique_files = unique_count(files.as_slice());

    if symbols.is_empty() {
        return "No test symbols are indexed yet.".to_owned();
    }

    if unique_files <= 1 {
        "Coverage is concentrated in a focused test module with tight feedback loops.".to_owned()
    } else {
        format!(
            "Coverage is distributed across {unique_files} files, which helps validate behavior across multiple layers."
        )
    }
}

fn top_symbol_narrative(symbols: &[SymbolInfo]) -> String {
    let mut sorted = symbols.to_vec();
    sorted.sort_by(|left, right| {
        right
            .dependents_count
            .cmp(&left.dependents_count)
            .then_with(|| left.name.cmp(&right.name))
    });

    let Some(top) = sorted.first() else {
        return "No dominant data component is available yet.".to_owned();
    };

    let intent = first_sentence(top.sir_intent.as_str());
    if intent.is_empty() {
        format!("{} is the main state component in this layer.", top.name)
    } else {
        format!("{} is the primary state component. {intent}", top.name)
    }
}

fn most_depended_layer_from_symbols(symbols: &[SymbolInfo]) -> &'static str {
    let mut counts = HashMap::<&'static str, usize>::new();

    for symbol in symbols {
        for dep in &symbol.dependencies {
            let layer = if matches_keyword(dep, &["db", "store", "cache", "repo", "state"]) {
                "Data"
            } else if matches_keyword(dep, &["frame", "codec", "wire", "proto"]) {
                "Wire Format"
            } else if matches_keyword(dep, &["client", "http", "socket", "provider"]) {
                "Connectors"
            } else {
                "Core Logic"
            };
            *counts.entry(layer).or_insert(0) += 1;
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(layer, _)| layer)
        .unwrap_or("Core Logic")
}

fn top_file_summaries(files: &[FileInfo], limit: usize) -> String {
    let summaries = files
        .iter()
        .take(limit)
        .map(|file| {
            if file.summary.trim().is_empty() {
                format!("{} contains {} components.", file.path, file.symbol_count)
            } else {
                file.summary.clone()
            }
        })
        .collect::<Vec<_>>();

    if summaries.is_empty() {
        "No file-level summary is available yet.".to_owned()
    } else {
        summaries.join(" ")
    }
}

fn file_summary_at(files: &[FileInfo], idx: usize) -> String {
    if let Some(file) = files.get(idx) {
        if file.summary.trim().is_empty() {
            format!("{} contains {} components.", file.path, file.symbol_count)
        } else {
            file.summary.clone()
        }
    } else {
        "".to_owned()
    }
}

fn format_dependency(dep: &Dependency) -> String {
    match dep
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(reason) => format!("{} ({reason})", dep.name),
        None => dep.name.clone(),
    }
}

fn dominant_kind(symbols: &[SymbolInfo]) -> String {
    let mut counts = HashMap::<String, usize>::new();
    for symbol in symbols {
        *counts.entry(symbol.kind.to_ascii_lowercase()).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(kind, _)| kind)
        .unwrap_or_else(|| "code".to_owned())
}

fn first_keyword(intent: &str) -> String {
    intent
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase()
}

fn first_sentence(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    for sep in ['.', '!', '?'] {
        if let Some(idx) = trimmed.find(sep) {
            return trimmed[..=idx].trim().to_owned();
        }
    }

    trimmed.to_owned()
}

fn matches_keyword(value: &str, terms: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    terms.iter().any(|term| lower.contains(term))
}

fn unique_count(values: &[&str]) -> usize {
    let mut seen = BTreeMap::<String, ()>::new();
    for value in values {
        seen.insert((*value).to_owned(), ());
    }
    seen.len()
}

fn join_human_list<S: AsRef<str>>(items: Vec<S>) -> String {
    let values = items
        .into_iter()
        .map(|item| item.as_ref().trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();

    match values.len() {
        0 => String::new(),
        1 => values[0].clone(),
        2 => format!("{} and {}", values[0], values[1]),
        _ => {
            let head = values[..values.len() - 1].join(", ");
            format!("{head}, and {}", values[values.len() - 1])
        }
    }
}
