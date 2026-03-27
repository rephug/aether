use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use aether_analysis::{
    BlastRadiusRequest, CouplingAnalyzer, HealthAnalyzer, HealthInclude, HealthRequest, RiskLevel,
};
use aether_core::normalize_path;
use aether_infer::summarize_text_with_config;
use aether_sir::SirAnnotation;
use aether_store::{
    DriftResultRecord, DriftStore, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    SymbolRelationStore,
};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::cli::EnhanceArgs;
use crate::enhance_templates::{estimate_tokens, render_enhanced_document};
use crate::search::{SearchMode, execute_search};

const EXTRACTION_WORD_LIMIT: usize = 500;
const MAX_EXPLICIT_MATCHES: usize = 3;
const MAX_SYMBOL_CONTEXTS: usize = 8;
const MAX_FILE_CONTEXTS: usize = 6;
const MAX_FILE_SUMMARIES: usize = 3;
const MAX_GRAPH_NEIGHBORS: usize = 5;
const MAX_CONCEPTS: usize = 5;
const MAX_COUPLING_NOTES: usize = 6;
const MAX_DRIFT_WARNINGS: usize = 4;
const JSON_ONLY_SYSTEM_PROMPT: &str = "Extract structured coding-task intent. Return JSON only with keys: target_symbols, target_files, concepts, task_type. Use arrays of strings. task_type must be one of: bug_fix, refactor, new_feature, test, documentation, investigation, general.";
const REWRITE_SYSTEM_PROMPT: &str = "You are a prompt engineering assistant for a software development AI agent.\nYou have been given a developer's original coding prompt and rich context from a codebase intelligence engine (AETHER).\n\nRewrite the original prompt into a clear, detailed, actionable prompt that:\n1. States the specific goal clearly\n2. References relevant files and symbols by name\n3. Mentions architectural constraints (coupling, health issues, drift)\n4. Suggests a logical approach based on the dependency graph\n5. Warns about edge cases from the SIR error modes\n\nKeep the rewritten prompt concise but thorough. Do not include the raw context dump — synthesize it into natural instructions.";

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "at", "be", "by", "for", "from", "how", "i", "in", "into", "is", "it",
    "me", "my", "of", "on", "or", "our", "please", "the", "this", "to", "we", "with",
];

const TASK_WORDS: &[&str] = &[
    "add",
    "bug",
    "debug",
    "document",
    "docs",
    "feature",
    "fix",
    "investigate",
    "refactor",
    "test",
    "write",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    BugFix,
    Refactor,
    NewFeature,
    Test,
    Documentation,
    Investigation,
    #[default]
    General,
}

impl TaskType {
    fn label(self) -> &'static str {
        match self {
            Self::BugFix => "bug fix",
            Self::Refactor => "refactor",
            Self::NewFeature => "new feature",
            Self::Test => "test",
            Self::Documentation => "documentation",
            Self::Investigation => "investigation",
            Self::General => "general",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhanceDocumentFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnhanceRequest {
    pub prompt: String,
    pub budget: usize,
    pub rewrite: bool,
    pub offline: bool,
    pub format: EnhanceDocumentFormat,
}

impl EnhanceRequest {
    pub fn new(prompt: impl Into<String>, budget: usize, rewrite: bool, offline: bool) -> Self {
        Self {
            prompt: prompt.into(),
            budget,
            rewrite,
            offline,
            format: EnhanceDocumentFormat::Text,
        }
    }

    pub fn with_format(mut self, format: EnhanceDocumentFormat) -> Self {
        self.format = format;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EnhanceIntent {
    pub raw_prompt: String,
    pub target_symbols: Vec<String>,
    pub target_files: Vec<String>,
    pub concepts: Vec<String>,
    pub task_type: TaskType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhanceResult {
    pub enhanced_prompt: String,
    pub resolved_symbols: Vec<String>,
    pub referenced_files: Vec<String>,
    pub rewrite_used: bool,
    pub token_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct AssembledEnhanceContext {
    pub task_type: TaskType,
    pub symbols: Vec<EnhanceSymbolContext>,
    pub files: Vec<EnhanceFileContext>,
    pub architectural_notes: Vec<String>,
    pub conventions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct EnhanceSymbolContext {
    pub symbol_id: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub generation_pass: Option<String>,
    pub intent: Option<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub side_effects: Vec<String>,
    pub error_modes: Vec<String>,
    pub dependencies: Vec<String>,
    pub callers: Vec<String>,
    pub health_score: Option<u32>,
    pub health_warnings: Vec<String>,
    pub drift_warnings: Vec<String>,
    pub contracts: Vec<String>,
    pub community_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct EnhanceFileContext {
    pub file_path: String,
    pub symbol_count: usize,
    pub top_symbols: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct AssemblyOutcome {
    context: AssembledEnhanceContext,
    resolved_symbols: Vec<String>,
    referenced_files: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawIntentPayload {
    #[serde(default)]
    target_symbols: Vec<String>,
    #[serde(default)]
    symbols: Vec<String>,
    #[serde(default)]
    target_files: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    concepts: Vec<String>,
    task_type: Option<String>,
}

pub fn run_enhance_command(workspace: &Path, args: EnhanceArgs) -> Result<()> {
    let request = EnhanceRequest::new(args.prompt.clone(), args.budget, args.rewrite, args.offline);
    let output_json = parse_cli_output(args.output.as_str())?;
    let result = if workspace.join(".aether").join("meta.sqlite").exists() {
        let store = SqliteStore::open_readonly(workspace).context("failed to open local store")?;
        enhance_prompt_core(workspace, &store, &request)?
    } else {
        warning_only_result(
            args.prompt.as_str(),
            "Workspace not indexed. Run `aether index` first.",
        )
    };

    let output = if output_json {
        serde_json::to_string_pretty(&result).context("failed to serialize enhance result")?
    } else {
        result.enhanced_prompt.clone()
    };

    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }

    for warning in &result.warnings {
        eprintln!("Warning: {warning}");
    }

    if args.clipboard {
        match copy_to_clipboard(output.as_str()) {
            Ok(Some(command)) => eprintln!("Copied output to clipboard via {command}."),
            Ok(None) => {
                eprintln!("Clipboard command not found. Tried pbcopy, wl-copy, xclip, and xsel.")
            }
            Err(err) => eprintln!("Clipboard copy failed: {err}"),
        }
    }

    Ok(())
}

pub fn enhance_prompt_core(
    workspace: &Path,
    store: &SqliteStore,
    request: &EnhanceRequest,
) -> Result<EnhanceResult> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(anyhow!("prompt must not be empty"));
    }
    if request.budget == 0 {
        return Err(anyhow!("budget must be greater than 0"));
    }

    let indexed_symbols = store
        .list_all_symbol_ids()
        .context("failed to list indexed symbols")?;
    if indexed_symbols.is_empty() {
        return Ok(warning_only_result(
            prompt,
            "Workspace is indexed but contains no symbols yet.",
        ));
    }

    let intent = extract_intent(workspace, store, request)?;
    let assembly = assemble_context(workspace, store, &intent)?;
    let rendered = render_enhanced_document(
        prompt,
        &assembly.context,
        request.budget,
        matches!(request.format, EnhanceDocumentFormat::Json),
    )?;

    let (enhanced_prompt, rewrite_used, mut warnings) = if request.rewrite {
        match rewrite_prompt(workspace, prompt, rendered.as_str()) {
            Ok(Some(rewritten)) if !rewritten.trim().is_empty() => {
                (rewritten, true, assembly.warnings)
            }
            Ok(_) => {
                let mut warnings = assembly.warnings;
                warnings.push("rewrite requested but inference returned no text".to_owned());
                (rendered, false, warnings)
            }
            Err(err) => {
                let mut warnings = assembly.warnings;
                warnings.push(format!("rewrite failed: {err}"));
                (rendered, false, warnings)
            }
        }
    } else {
        (rendered, false, assembly.warnings)
    };

    let token_count = estimate_tokens(enhanced_prompt.as_str());
    warnings.sort();
    warnings.dedup();

    Ok(EnhanceResult {
        enhanced_prompt,
        resolved_symbols: assembly.resolved_symbols,
        referenced_files: assembly.referenced_files,
        rewrite_used,
        token_count,
        warnings,
    })
}

pub(crate) fn extract_intent_via_keywords(
    workspace: &Path,
    store: &SqliteStore,
    prompt: &str,
) -> Result<EnhanceIntent> {
    let mut concepts = Vec::<String>::new();
    let mut target_symbols = Vec::<String>::new();
    let mut target_files = Vec::<String>::new();
    let mut seen_concepts = HashSet::<String>::new();
    let mut seen_symbols = HashSet::<String>::new();
    let mut seen_files = HashSet::<String>::new();
    let tokens = tokenize_prompt(prompt);

    for token in tokens.iter().take(64) {
        let lower = token.to_ascii_lowercase();
        if STOP_WORDS.contains(&lower.as_str()) {
            continue;
        }

        if let Some(path) = resolve_file_token(workspace, store, token)? {
            if seen_files.insert(path.clone()) {
                target_files.push(path);
            }
            continue;
        }

        let looks_symbolic = token.contains("::")
            || token.contains('.')
            || token.chars().any(|ch| ch.is_ascii_uppercase());
        if looks_symbolic {
            let matches = resolve_symbol_query(store, token, MAX_EXPLICIT_MATCHES)?;
            if !matches.is_empty() {
                if seen_symbols.insert(token.clone()) {
                    target_symbols.push(token.clone());
                }
                continue;
            }
        }

        let matches = store
            .search_symbols(token, 5)
            .with_context(|| format!("failed to search symbols for token '{token}'"))?;
        if !matches.is_empty() {
            if seen_symbols.insert(token.clone()) {
                target_symbols.push(token.clone());
            }
            continue;
        }

        if token.len() >= 4
            && !TASK_WORDS.contains(&lower.as_str())
            && seen_concepts.insert(lower.clone())
        {
            concepts.push(lower);
        }
    }

    Ok(EnhanceIntent {
        raw_prompt: prompt.trim().to_owned(),
        target_symbols,
        target_files,
        concepts,
        task_type: classify_task_type(prompt),
    })
}

fn extract_intent(
    workspace: &Path,
    store: &SqliteStore,
    request: &EnhanceRequest,
) -> Result<EnhanceIntent> {
    if request.offline || prompt_has_explicit_targets(request.prompt.as_str()) {
        return extract_intent_via_keywords(workspace, store, request.prompt.as_str());
    }

    match extract_intent_via_llm(workspace, request.prompt.as_str()) {
        Ok(intent) => Ok(intent),
        Err(_) => extract_intent_via_keywords(workspace, store, request.prompt.as_str()),
    }
}

fn extract_intent_via_llm(workspace: &Path, prompt: &str) -> Result<EnhanceIntent> {
    let extraction_prompt = extraction_user_prompt(prompt);
    let response = block_on_task(summarize_text_with_config(
        workspace,
        JSON_ONLY_SYSTEM_PROMPT,
        extraction_prompt.as_str(),
    ))
    .context("intent extraction runtime failed")??
    .ok_or_else(|| anyhow!("intent extraction returned no text"))?;
    let json_text = extract_json_object(response.as_str())
        .ok_or_else(|| anyhow!("intent extraction did not return JSON"))?;
    let payload: RawIntentPayload =
        serde_json::from_str(json_text.as_str()).context("failed to parse extraction JSON")?;

    Ok(EnhanceIntent {
        raw_prompt: prompt.trim().to_owned(),
        target_symbols: normalize_string_list(payload.target_symbols, payload.symbols, 8),
        target_files: normalize_string_list(payload.target_files, payload.files, 8),
        concepts: normalize_single_list(payload.concepts, MAX_CONCEPTS),
        task_type: payload
            .task_type
            .as_deref()
            .map(parse_task_type)
            .unwrap_or_else(|| classify_task_type(prompt)),
    })
}

fn assemble_context(
    workspace: &Path,
    store: &SqliteStore,
    intent: &EnhanceIntent,
) -> Result<AssemblyOutcome> {
    let mut warnings = Vec::<String>::new();
    let mut resolved_symbol_records = Vec::<SymbolRecord>::new();
    let mut seen_symbol_ids = HashSet::<String>::new();
    let mut referenced_files = Vec::<String>::new();
    let mut seen_files = HashSet::<String>::new();
    let mut file_symbol_cache = HashMap::<String, Vec<SymbolRecord>>::new();

    for query in intent.target_symbols.iter().take(MAX_SYMBOL_CONTEXTS) {
        let matches = resolve_symbol_query(store, query, MAX_EXPLICIT_MATCHES)?;
        if matches.is_empty() {
            warnings.push(format!("no symbol matched '{query}'"));
            continue;
        }
        for record in matches {
            if seen_symbol_ids.insert(record.id.clone()) {
                resolved_symbol_records.push(record);
            }
        }
    }

    for file in intent.target_files.iter().take(MAX_FILE_CONTEXTS) {
        match resolve_file_token(workspace, store, file)? {
            Some(path) => {
                let records = store
                    .list_symbols_for_file(path.as_str())
                    .with_context(|| format!("failed to list symbols for {path}"))?;
                if records.is_empty() {
                    warnings.push(format!("no indexed symbols found for file '{file}'"));
                    continue;
                }
                if seen_files.insert(path.clone()) {
                    referenced_files.push(path.clone());
                }
                file_symbol_cache.insert(path, records);
            }
            None => warnings.push(format!("no indexed file matched '{file}'")),
        }
    }

    for concept in intent.concepts.iter().take(MAX_CONCEPTS) {
        let execution = execute_search(workspace, concept, 3, SearchMode::Hybrid)
            .with_context(|| format!("failed to search concept '{concept}'"))?;
        if let Some(reason) = execution.fallback_reason.as_deref() {
            warnings.push(format!(
                "semantic search fallback for '{concept}': {reason}"
            ));
        }
        if execution.matches.is_empty() {
            warnings.push(format!("no concept matches found for '{concept}'"));
            continue;
        }

        for entry in execution.matches.iter().take(3) {
            let Some(record) = store
                .get_symbol_record(entry.symbol_id.as_str())
                .with_context(|| format!("failed to load symbol record {}", entry.symbol_id))?
            else {
                continue;
            };
            if seen_symbol_ids.insert(record.id.clone()) {
                resolved_symbol_records.push(record);
            }
        }
    }

    for record in &resolved_symbol_records {
        if seen_files.insert(record.file_path.clone()) {
            referenced_files.push(record.file_path.clone());
        }
    }

    for file_path in referenced_files.iter().take(MAX_FILE_CONTEXTS) {
        if file_symbol_cache.contains_key(file_path) {
            continue;
        }
        let records = store
            .list_symbols_for_file(file_path.as_str())
            .with_context(|| format!("failed to list symbols for {file_path}"))?;
        if !records.is_empty() {
            file_symbol_cache.insert(file_path.clone(), records);
        }
    }

    let health_by_symbol =
        match collect_health_by_symbol(workspace, store, resolved_symbol_records.as_slice()) {
            Ok(map) => map,
            Err(err) => {
                warnings.push(format!("health analysis unavailable: {err}"));
                HashMap::new()
            }
        };

    let drift_records = match store.list_drift_results(false) {
        Ok(records) => records,
        Err(err) => {
            warnings.push(format!("drift warnings unavailable: {err}"));
            Vec::new()
        }
    };
    let community_by_symbol = match store.list_latest_community_snapshot() {
        Ok(records) => records
            .into_iter()
            .map(|entry| (entry.symbol_id, entry.community_id))
            .collect::<HashMap<_, _>>(),
        Err(err) => {
            warnings.push(format!("community data unavailable: {err}"));
            HashMap::new()
        }
    };

    let mut symbol_contexts = Vec::<EnhanceSymbolContext>::new();
    for record in resolved_symbol_records.iter().take(MAX_SYMBOL_CONTEXTS) {
        symbol_contexts.push(build_symbol_context(
            store,
            record,
            health_by_symbol.get(record.id.as_str()),
            drift_records.as_slice(),
            community_by_symbol.get(record.id.as_str()).copied(),
        )?);
    }

    let mut file_contexts = Vec::<EnhanceFileContext>::new();
    for file_path in referenced_files.iter().take(MAX_FILE_CONTEXTS) {
        let records = file_symbol_cache
            .get(file_path)
            .cloned()
            .unwrap_or_default();
        file_contexts.push(build_file_context(
            store,
            file_path.as_str(),
            records.as_slice(),
        )?);
    }

    let mut architectural_notes = collect_coupling_notes(workspace, referenced_files.as_slice());
    if architectural_notes.is_empty() && !referenced_files.is_empty() {
        warnings.push("coupling data unavailable or no coupling edges were recorded".to_owned());
    }

    let drift_notes = collect_workspace_drift_notes(
        drift_records.as_slice(),
        resolved_symbol_records.as_slice(),
        referenced_files.as_slice(),
    );
    architectural_notes.extend(drift_notes);

    if let Some(note) = community_note(symbol_contexts.as_slice()) {
        architectural_notes.push(note);
    }
    if symbol_contexts.is_empty() && file_contexts.is_empty() {
        architectural_notes.push(format!(
            "No direct symbol or file matches were found. Task classified as {}.",
            intent.task_type.label()
        ));
    }

    let conventions = collect_conventions(intent.task_type, symbol_contexts.as_slice());

    Ok(AssemblyOutcome {
        context: AssembledEnhanceContext {
            task_type: intent.task_type,
            symbols: symbol_contexts,
            files: file_contexts,
            architectural_notes,
            conventions,
        },
        resolved_symbols: resolved_symbol_records
            .into_iter()
            .take(MAX_SYMBOL_CONTEXTS)
            .map(|record| record.qualified_name)
            .collect(),
        referenced_files: referenced_files
            .into_iter()
            .take(MAX_FILE_CONTEXTS)
            .collect(),
        warnings,
    })
}

fn build_symbol_context(
    store: &SqliteStore,
    record: &SymbolRecord,
    health_entry: Option<&aether_analysis::SymbolHealthEntry>,
    drift_records: &[DriftResultRecord],
    community_id: Option<i64>,
) -> Result<EnhanceSymbolContext> {
    let sir = read_sir_annotation(store, record.id.as_str());
    let meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to load SIR metadata for {}", record.id))?;
    let dependencies = read_dependencies(store, record.id.as_str())?;
    let callers = read_callers(store, record.qualified_name.as_str())?;
    let contracts = store
        .list_active_contracts_for_symbol(record.id.as_str())
        .with_context(|| format!("failed to list contracts for {}", record.id))?
        .into_iter()
        .map(|contract| format!("{} {}", contract.clause_type, contract.clause_text))
        .take(3)
        .collect::<Vec<_>>();
    let drift_warnings = drift_records
        .iter()
        .filter(|entry| entry.symbol_id == record.id)
        .take(MAX_DRIFT_WARNINGS)
        .map(|entry| {
            entry
                .drift_summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| format!("{} drift detected", entry.drift_type))
        })
        .collect::<Vec<_>>();

    Ok(EnhanceSymbolContext {
        symbol_id: record.id.clone(),
        qualified_name: record.qualified_name.clone(),
        kind: record.kind.clone(),
        file_path: record.file_path.clone(),
        generation_pass: meta.map(|entry| entry.generation_pass),
        intent: sir
            .as_ref()
            .map(|entry| first_sentence(entry.intent.as_str())),
        inputs: sir
            .as_ref()
            .map(|entry| entry.inputs.iter().take(3).cloned().collect())
            .unwrap_or_default(),
        outputs: sir
            .as_ref()
            .map(|entry| entry.outputs.iter().take(3).cloned().collect())
            .unwrap_or_default(),
        side_effects: sir
            .as_ref()
            .map(|entry| entry.side_effects.iter().take(4).cloned().collect())
            .unwrap_or_default(),
        error_modes: sir
            .as_ref()
            .map(|entry| entry.error_modes.iter().take(4).cloned().collect())
            .unwrap_or_default(),
        dependencies,
        callers,
        health_score: health_entry
            .map(|entry| (entry.risk_score * 100.0).round().clamp(0.0, 100.0) as u32),
        health_warnings: health_entry
            .map(|entry| entry.risk_factors.iter().take(3).cloned().collect())
            .unwrap_or_default(),
        drift_warnings,
        contracts,
        community_id,
    })
}

fn build_file_context(
    store: &SqliteStore,
    file_path: &str,
    records: &[SymbolRecord],
) -> Result<EnhanceFileContext> {
    let mut top_symbols = Vec::<String>::new();
    for record in records.iter().take(MAX_FILE_SUMMARIES) {
        let summary = read_sir_annotation(store, record.id.as_str())
            .map(|sir| first_sentence(sir.intent.as_str()))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                format!(
                    "{} {}",
                    record.kind,
                    leaf_name(record.qualified_name.as_str())
                )
            });
        top_symbols.push(format!("`{}` — {}", record.qualified_name, summary));
    }

    Ok(EnhanceFileContext {
        file_path: file_path.to_owned(),
        symbol_count: records.len(),
        top_symbols,
    })
}

fn collect_health_by_symbol(
    workspace: &Path,
    store: &SqliteStore,
    records: &[SymbolRecord],
) -> Result<HashMap<String, aether_analysis::SymbolHealthEntry>> {
    if records.is_empty() {
        return Ok(HashMap::new());
    }

    let total_symbols = store
        .list_all_symbol_ids()
        .context("failed to list symbols for health analysis")?
        .len();
    if total_symbols == 0 {
        return Ok(HashMap::new());
    }

    let analyzer =
        HealthAnalyzer::new(workspace).context("failed to initialize health analyzer")?;
    let request = HealthRequest {
        include: vec![
            HealthInclude::CriticalSymbols,
            HealthInclude::Cycles,
            HealthInclude::RiskHotspots,
        ],
        limit: total_symbols as u32,
        min_risk: 0.0,
    };

    let report =
        block_on_task(analyzer.analyze(&request)).context("health analysis runtime failed")??;
    Ok(report
        .critical_symbols
        .into_iter()
        .map(|entry| (entry.symbol_id.clone(), entry))
        .collect())
}

fn collect_coupling_notes(workspace: &Path, files: &[String]) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }

    let analyzer = match CouplingAnalyzer::new(workspace) {
        Ok(analyzer) => analyzer,
        Err(_) => return Vec::new(),
    };
    // Intentional best effort: coupling notes are optional prompt enrichment only.
    let file_set = files.iter().cloned().collect::<HashSet<_>>();
    let mut notes = BTreeSet::<String>::new();

    for file in files {
        let result = analyzer.blast_radius(BlastRadiusRequest {
            file_path: file.clone(),
            min_risk: RiskLevel::Low,
            auto_mine: false,
        });
        let Ok(result) = result else {
            continue;
        };
        for entry in result.coupled_files.into_iter().take(3) {
            if file_set.contains(entry.file.as_str()) || entry.fused_score < 0.2 {
                continue;
            }
            notes.insert(format!(
                "`{}` is coupled with `{}` ({:?}, {:.2})",
                file, entry.file, entry.risk_level, entry.fused_score
            ));
            if notes.len() >= MAX_COUPLING_NOTES {
                return notes.into_iter().collect();
            }
        }
    }

    notes.into_iter().collect()
}

fn collect_workspace_drift_notes(
    drift_records: &[DriftResultRecord],
    resolved_symbols: &[SymbolRecord],
    referenced_files: &[String],
) -> Vec<String> {
    let symbol_ids = resolved_symbols
        .iter()
        .map(|record| record.id.as_str())
        .collect::<HashSet<_>>();
    let file_paths = referenced_files
        .iter()
        .map(|value| value.as_str())
        .collect::<HashSet<_>>();

    drift_records
        .iter()
        .filter(|entry| {
            symbol_ids.contains(entry.symbol_id.as_str())
                || file_paths.contains(entry.file_path.as_str())
        })
        .take(MAX_DRIFT_WARNINGS)
        .map(|entry| {
            entry
                .drift_summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| format!("{} drift on `{}`", entry.drift_type, entry.symbol_name))
        })
        .collect()
}

fn community_note(symbols: &[EnhanceSymbolContext]) -> Option<String> {
    let communities = symbols
        .iter()
        .filter_map(|symbol| symbol.community_id)
        .collect::<BTreeSet<_>>();
    match communities.len() {
        0 => None,
        1 => communities
            .iter()
            .next()
            .map(|community| format!("Resolved symbols sit in community {community}, suggesting a localized change boundary.")),
        _ => Some(format!(
            "Resolved symbols span communities {}. Treat this as a cross-boundary change.",
            communities
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn collect_conventions(task_type: TaskType, symbols: &[EnhanceSymbolContext]) -> Vec<String> {
    let mut notes = Vec::<String>::new();
    notes.push(format!(
        "Treat this as a {} and preserve the existing interfaces unless the cited context forces a wider change.",
        task_type.label()
    ));

    if symbols.iter().any(|symbol| !symbol.error_modes.is_empty()) {
        notes.push("Preserve or explicitly update the recorded error-handling paths instead of silently changing them.".to_owned());
    }
    if symbols.iter().any(|symbol| !symbol.side_effects.is_empty()) {
        notes.push(
            "Preserve the recorded side effects unless the task explicitly changes behavior."
                .to_owned(),
        );
    }
    if symbols.iter().any(|symbol| !symbol.contracts.is_empty()) {
        notes.push("Respect active contract clauses on the targeted symbols.".to_owned());
    }

    notes
}

fn read_dependencies(store: &SqliteStore, symbol_id: &str) -> Result<Vec<String>> {
    let mut entries = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();
    for edge in store
        .get_dependencies(symbol_id)
        .with_context(|| format!("failed to load dependencies for {symbol_id}"))?
        .into_iter()
        .take(MAX_GRAPH_NEIGHBORS)
    {
        if seen.insert(edge.target_qualified_name.clone()) {
            entries.push(edge.target_qualified_name);
        }
    }
    Ok(entries)
}

fn read_callers(store: &SqliteStore, qualified_name: &str) -> Result<Vec<String>> {
    let mut entries = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();
    for edge in store
        .get_callers(qualified_name)
        .with_context(|| format!("failed to load callers for {qualified_name}"))?
        .into_iter()
        .take(MAX_GRAPH_NEIGHBORS)
    {
        let label = store
            .get_symbol_record(edge.source_id.as_str())
            .with_context(|| format!("failed to load caller {}", edge.source_id))?
            .map(|record| record.qualified_name)
            .unwrap_or(edge.source_id);
        if seen.insert(label.clone()) {
            entries.push(label);
        }
    }
    Ok(entries)
}

fn resolve_symbol_query(
    store: &SqliteStore,
    query: &str,
    limit: usize,
) -> Result<Vec<SymbolRecord>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut records = Vec::<SymbolRecord>::new();
    let mut seen = HashSet::<String>::new();

    if let Some(record) = store
        .get_symbol_record(query)
        .with_context(|| format!("failed to resolve symbol id {query}"))?
        && seen.insert(record.id.clone())
    {
        records.push(record);
    }

    if let Some(record) = store
        .get_symbol_by_qualified_name(query)
        .with_context(|| format!("failed to resolve symbol name {query}"))?
        && seen.insert(record.id.clone())
    {
        records.push(record);
    }

    for candidate in store
        .find_symbol_search_results_by_qualified_name(query)
        .with_context(|| format!("failed to search symbol name {query}"))?
        .into_iter()
        .take(limit)
    {
        let Some(record) = store
            .get_symbol_record(candidate.symbol_id.as_str())
            .with_context(|| format!("failed to load symbol {}", candidate.symbol_id))?
        else {
            continue;
        };
        if seen.insert(record.id.clone()) {
            records.push(record);
        }
    }

    if records.len() >= limit {
        return Ok(records);
    }

    for candidate in store
        .search_symbols(query, limit as u32)
        .with_context(|| format!("failed to fuzzy-search symbol {query}"))?
    {
        let Some(record) = store
            .get_symbol_record(candidate.symbol_id.as_str())
            .with_context(|| format!("failed to load symbol {}", candidate.symbol_id))?
        else {
            continue;
        };
        if seen.insert(record.id.clone()) {
            records.push(record);
        }
        if records.len() >= limit {
            break;
        }
    }

    Ok(records)
}

fn resolve_file_token(
    workspace: &Path,
    store: &SqliteStore,
    token: &str,
) -> Result<Option<String>> {
    if !is_probable_path(token) {
        return Ok(None);
    }

    let mut candidates = Vec::<String>::new();
    let normalized = normalize_path(token.trim());
    if !normalized.is_empty() {
        candidates.push(normalized.clone());
    }
    if let Some(stripped) = normalized.strip_prefix("./") {
        candidates.push(stripped.to_owned());
    }
    let path = Path::new(token.trim());
    if path.is_absolute()
        && let Ok(relative) = path.strip_prefix(workspace)
    {
        candidates.push(normalize_path(relative.to_string_lossy().as_ref()));
    }

    let mut seen = HashSet::<String>::new();
    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        let records = store
            .list_symbols_for_file(candidate.as_str())
            .with_context(|| format!("failed to list symbols for {candidate}"))?;
        if !records.is_empty() {
            return Ok(Some(candidate));
        }
    }

    Ok(None)
}

fn read_sir_annotation(store: &SqliteStore, symbol_id: &str) -> Option<SirAnnotation> {
    let blob = match store.read_sir_blob(symbol_id) {
        Ok(Some(blob)) => blob,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(
                symbol_id,
                error = %err,
                "failed to read SIR blob for enhancement context"
            );
            return None;
        }
    };
    match serde_json::from_str::<SirAnnotation>(&blob) {
        Ok(sir) => Some(sir),
        Err(err) => {
            tracing::warn!(
                symbol_id,
                error = %err,
                "failed to parse SIR blob for enhancement context"
            );
            None
        }
    }
}

fn extraction_user_prompt(prompt: &str) -> String {
    let trimmed = trim_to_word_limit(prompt, EXTRACTION_WORD_LIMIT);
    format!(
        "Given this coding task prompt, extract:\n1. Any specific symbol names (functions, structs, modules) mentioned or implied\n2. Any file paths mentioned or implied\n3. Key concepts or domains referenced\n4. The task type (bug_fix, refactor, new_feature, test, documentation, investigation, general)\n\nRespond in JSON only. No explanation.\n\nPrompt: \"{trimmed}\""
    )
}

fn rewrite_prompt(
    workspace: &Path,
    original_prompt: &str,
    rendered: &str,
) -> Result<Option<String>> {
    let user_prompt = format!("Original prompt:\n{original_prompt}\n\nContext:\n{rendered}\n");
    block_on_task(summarize_text_with_config(
        workspace,
        REWRITE_SYSTEM_PROMPT,
        user_prompt.as_str(),
    ))
    .context("rewrite runtime failed")?
    .context("rewrite inference failed")
}

fn prompt_has_explicit_targets(prompt: &str) -> bool {
    prompt.contains("::")
        || prompt.contains('/')
        || prompt.contains(".rs")
        || prompt.contains(".toml")
        || prompt.contains('`')
}

fn tokenize_prompt(prompt: &str) -> Vec<String> {
    let mut tokens = Vec::<String>::new();
    let mut current = String::new();

    for ch in prompt.chars() {
        if ch.is_alphanumeric() || matches!(ch, '_' | ':' | '/' | '.' | '-') {
            current.push(ch);
        } else if !current.is_empty() {
            push_clean_token(&mut tokens, &mut current);
        }
    }

    if !current.is_empty() {
        push_clean_token(&mut tokens, &mut current);
    }

    tokens
}

fn push_clean_token(tokens: &mut Vec<String>, current: &mut String) {
    let cleaned = current
        .trim_matches(|ch: char| matches!(ch, '.' | ':' | '-' | '/'))
        .trim_matches('`')
        .to_owned();
    if !cleaned.is_empty() {
        tokens.push(cleaned);
    }
    current.clear();
}

fn normalize_string_list(
    primary: Vec<String>,
    secondary: Vec<String>,
    limit: usize,
) -> Vec<String> {
    normalize_single_list(primary.into_iter().chain(secondary).collect(), limit)
}

fn normalize_single_list(values: Vec<String>, limit: usize) -> Vec<String> {
    let mut seen = HashSet::<String>::new();
    values
        .into_iter()
        .filter_map(|value| {
            let cleaned = value.trim().trim_matches('`').to_owned();
            if cleaned.is_empty() || !seen.insert(cleaned.clone()) {
                None
            } else {
                Some(cleaned)
            }
        })
        .take(limit)
        .collect()
}

fn parse_task_type(raw: &str) -> TaskType {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bug_fix" | "bugfix" => TaskType::BugFix,
        "refactor" => TaskType::Refactor,
        "new_feature" | "feature" => TaskType::NewFeature,
        "test" | "tests" => TaskType::Test,
        "documentation" | "docs" | "doc" => TaskType::Documentation,
        "investigation" | "investigate" => TaskType::Investigation,
        _ => TaskType::General,
    }
}

fn classify_task_type(prompt: &str) -> TaskType {
    let lower = prompt.to_ascii_lowercase();
    if ["bug", "fix", "broken", "regression", "repair"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::BugFix;
    }
    if ["refactor", "cleanup", "simplify", "split"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::Refactor;
    }
    if ["feature", "add", "implement", "build"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::NewFeature;
    }
    if ["test", "coverage", "assert", "spec"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::Test;
    }
    if ["doc", "document", "readme", "comment"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::Documentation;
    }
    if ["investigate", "debug", "analyze", "trace", "why"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return TaskType::Investigation;
    }
    TaskType::General
}

fn trim_to_word_limit(value: &str, limit: usize) -> String {
    value
        .split_whitespace()
        .take(limit)
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_json_object(value: &str) -> Option<String> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (end > start).then(|| value[start..=end].to_owned())
}

fn first_sentence(value: &str) -> String {
    value
        .split(['\n', '.', '!', '?'])
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn leaf_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .or_else(|| qualified_name.rsplit('.').next())
        .unwrap_or(qualified_name)
        .to_owned()
}

fn is_probable_path(token: &str) -> bool {
    token.contains('/')
        || token.ends_with(".rs")
        || token.ends_with(".toml")
        || token.ends_with(".md")
        || token.ends_with(".json")
        || token.ends_with(".yaml")
        || token.ends_with(".yml")
}

fn block_on_task<F, T>(future: F) -> Result<T>
where
    F: Future<Output = T>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    Ok(runtime.block_on(future))
}

fn parse_cli_output(raw: &str) -> Result<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "text" => Ok(false),
        "json" => Ok(true),
        other => Err(anyhow!(
            "invalid output format '{other}', expected one of: text, json"
        )),
    }
}

fn warning_only_result(prompt: &str, warning: &str) -> EnhanceResult {
    EnhanceResult {
        enhanced_prompt: prompt.trim().to_owned(),
        resolved_symbols: Vec::new(),
        referenced_files: Vec::new(),
        rewrite_used: false,
        token_count: estimate_tokens(prompt.trim()),
        warnings: vec![warning.to_owned()],
    }
}

fn copy_to_clipboard(text: &str) -> io::Result<Option<&'static str>> {
    let commands = [
        ("pbcopy", &[][..]),
        ("wl-copy", &[][..]),
        ("xclip", &["-selection", "clipboard"][..]),
        ("xsel", &["--clipboard", "--input"][..]),
    ];

    let mut last_error = None;
    for (program, args) in commands {
        match try_clipboard_command(program, args, text) {
            Ok(true) => return Ok(Some(program)),
            Ok(false) => continue,
            Err(err) => last_error = Some(err),
        }
    }

    if let Some(err) = last_error {
        return Err(err);
    }
    Ok(None)
}

fn try_clipboard_command(program: &str, args: &[&str], text: &str) -> io::Result<bool> {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }

    let status = child.wait()?;
    if status.success() {
        Ok(true)
    } else {
        Err(io::Error::other(format!(
            "{program} exited with status {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{
        SirMetaRecord, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRelationStore,
    };
    use tempfile::tempdir;
    use tracing::dispatcher::{self, Dispatch};
    use tracing_subscriber::fmt::MakeWriter;

    use super::{
        EnhanceDocumentFormat, EnhanceRequest, TaskType, enhance_prompt_core,
        extract_intent_via_keywords, read_sir_annotation,
    };

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(self.0.clone())
        }
    }

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs<T>(run: impl FnOnce() -> T) -> (T, String) {
        let buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_target(false)
            .with_writer(buffer.clone())
            .finish();
        let result = dispatcher::with_default(&Dispatch::new(subscriber), run);
        let logs = String::from_utf8(buffer.0.lock().expect("log buffer lock").clone())
            .expect("utf8 logs");
        (result, logs)
    }

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "gemini"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"

[health]
enabled = true

[coupling]
enabled = true
"#,
        )
        .expect("write config");
    }

    fn seed_symbol(
        store: &SqliteStore,
        id: &str,
        file_path: &str,
        qualified_name: &str,
        kind: &str,
        intent: &str,
    ) {
        store
            .upsert_symbol(aether_store::SymbolRecord {
                id: id.to_owned(),
                file_path: file_path.to_owned(),
                language: "rust".to_owned(),
                kind: kind.to_owned(),
                qualified_name: qualified_name.to_owned(),
                signature_fingerprint: format!("sig-{id}"),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
        let sir_json = serde_json::json!({
            "intent": intent,
            "inputs": ["request"],
            "outputs": ["result"],
            "side_effects": ["writes state"],
            "dependencies": ["db"],
            "error_modes": ["io failure"],
            "confidence": 0.81
        })
        .to_string();
        store
            .write_sir_blob(id, sir_json.as_str())
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: id.to_owned(),
                sir_hash: format!("hash-{id}"),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "triage".to_owned(),
                reasoning_trace: Some("mock reasoning".to_owned()),
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_100,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_100,
            })
            .expect("upsert meta");
    }

    fn seed_workspace(workspace: &Path) -> SqliteStore {
        write_test_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("mkdir src");
        fs::write(
            workspace.join("src/lib.rs"),
            "pub fn login() {}\npub fn auth_helper() {}\n",
        )
        .expect("write lib");
        let store = SqliteStore::open(workspace).expect("open store");
        seed_symbol(
            &store,
            "sym_login",
            "src/lib.rs",
            "demo::login",
            "function",
            "Handles login requests and validates credentials.",
        );
        seed_symbol(
            &store,
            "sym_helper",
            "src/lib.rs",
            "demo::auth_helper",
            "function",
            "Provides helper routines for authentication state.",
        );
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: "sym_login".to_owned(),
                    target_qualified_name: "demo::auth_helper".to_owned(),
                    edge_kind: EdgeKind::DependsOn,
                    file_path: "src/lib.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: "sym_helper".to_owned(),
                    target_qualified_name: "demo::login".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
            ])
            .expect("upsert edges");
        store
    }

    #[test]
    fn keyword_extraction_classifies_bug_fix_and_preserves_login_concept() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        let intent =
            extract_intent_via_keywords(workspace, &store, "fix the login bug").expect("intent");

        assert_eq!(intent.task_type, TaskType::BugFix);
        assert!(intent.concepts.iter().any(|concept| concept == "login"));
    }

    #[test]
    fn template_output_contains_expected_sections() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = seed_workspace(workspace);
        let result = enhance_prompt_core(
            workspace,
            &store,
            &EnhanceRequest {
                prompt: "fix the login bug".to_owned(),
                budget: 8_000,
                rewrite: false,
                offline: true,
                format: EnhanceDocumentFormat::Text,
            },
        )
        .expect("enhance");

        assert!(result.enhanced_prompt.contains("## Enhanced Prompt"));
        assert!(result.enhanced_prompt.contains("### Target Symbols"));
        assert!(result.enhanced_prompt.contains("### Related Files"));
    }

    #[test]
    fn empty_workspace_returns_original_prompt_with_warning() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let store = SqliteStore::open(workspace).expect("open store");
        let result = enhance_prompt_core(
            workspace,
            &store,
            &EnhanceRequest {
                prompt: "investigate auth".to_owned(),
                budget: 8_000,
                rewrite: false,
                offline: true,
                format: EnhanceDocumentFormat::Text,
            },
        )
        .expect("enhance");

        assert_eq!(result.enhanced_prompt, "investigate auth");
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("contains no symbols"))
        );
    }

    #[test]
    fn token_budget_truncates_large_context() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = seed_workspace(workspace);
        seed_symbol(
            &store,
            "sym_extra",
            "src/lib.rs",
            "demo::extra_long_context",
            "function",
            "Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent. Produces a very long narrative intent.",
        );
        let result = enhance_prompt_core(
            workspace,
            &store,
            &EnhanceRequest {
                prompt: "refactor login and auth helper".to_owned(),
                budget: 120,
                rewrite: false,
                offline: true,
                format: EnhanceDocumentFormat::Text,
            },
        )
        .expect("enhance");

        assert!(result.token_count <= 120);
        assert!(result.enhanced_prompt.contains("[... truncated ...]"));
    }

    #[test]
    fn read_sir_annotation_returns_none_and_logs_on_parse_error() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .write_sir_blob("sym-invalid", "{invalid")
            .expect("write invalid sir");

        let (sir, logs) = capture_logs(|| read_sir_annotation(&store, "sym-invalid"));

        assert!(sir.is_none());
        assert!(logs.contains("failed to parse SIR blob for enhancement context"));
        assert!(logs.contains("sym-invalid"));
    }
}
