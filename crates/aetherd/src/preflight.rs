//! Deterministic, provider-free task briefing shared by CLI and MCP.
//!
//! This is a bounded structural map, not proof that the index or its inferred
//! relationships are complete. Opening this service never initializes a store.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use aether_core::{EdgeKind, GitContext, same_language_family};
use aether_store::{
    ProjectNoteStore, RetirementCleanupStore, SirHistoryStore, SqliteStore, SymbolHistoryStore,
    SymbolRecord, TestIntentStore,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

const MAX_FILES: usize = 5_000;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const GENERATOR: &str = "aether-structural-preflight-v1";

#[derive(Debug, Clone)]
pub struct PreflightRequest {
    pub task: String,
    pub scope: Option<String>,
    /// UTF-8 serialized bytes / 4, rounded up; not a tokenizer guarantee.
    pub max_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightMap {
    pub generator: String,
    pub hash_algorithm: String,
    pub revision: Option<String>,
    /// Hash of the bounded source inventory, including file names and contents.
    pub source_hash: String,
    /// Source hash plus HEAD; includes untracked supported source, excludes stores.
    pub worktree_hash: String,
    pub config_hash: String,
    pub task_hash: String,
    /// BLAKE3 of canonical JSON excluding map_hash and estimated_tokens.
    pub map_hash: String,
    pub context: String,
    pub omitted_items: BTreeMap<String, usize>,
    pub estimated_tokens: usize,
    pub estimate_basis: String,
}

struct SourceFile {
    path: String,
    text: String,
}

struct Section {
    title: &'static str,
    empty: &'static str,
    items: Vec<String>,
}

/// Assemble and budget the ordinary JSON representation. MCP uses the same
/// assembler with a measurement of its actual SDK tool-result representation.
pub fn assemble_preflight(workspace: &Path, request: &PreflightRequest) -> Result<PreflightMap> {
    assemble_preflight_measured(workspace, request, |map| {
        Ok(serde_json::to_vec(map)?.len().div_ceil(4))
    })
}

pub fn assemble_preflight_measured(
    workspace: &Path,
    request: &PreflightRequest,
    measure: impl Fn(&PreflightMap) -> Result<usize>,
) -> Result<PreflightMap> {
    if request.task.trim().is_empty() || request.task.len() > 32_000 {
        bail!("task must contain 1 to 32000 UTF-8 bytes");
    }
    if !(512..=100_000).contains(&request.max_tokens) {
        bail!("max_tokens must be between 512 and 100000 estimated tokens");
    }
    let scope = request.scope.as_deref().unwrap_or("");
    if scope.len() > 400
        || Path::new(scope)
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!("scope must be a workspace-relative path without parent components");
    }
    let normalized_scope = Path::new(scope)
        .components()
        .filter_map(|part| {
            if let Component::Normal(name) = part {
                Some(name.to_string_lossy())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    let scope = normalized_scope.as_str();
    let workspace = workspace
        .canonicalize()
        .context("resolve pre-flight workspace")?;
    let (sources, excluded) = read_sources(&workspace, scope)?;
    let mut source_hasher = blake3::Hasher::new();
    for source in &sources {
        hash_field(&mut source_hasher, source.path.as_bytes());
        hash_field(&mut source_hasher, source.text.as_bytes());
    }
    let source_hash = source_hasher.finalize().to_hex().to_string();
    let git = GitContext::open(&workspace);
    let revision = git.as_ref().and_then(GitContext::head_commit_hash);
    let worktree_hash = blake3::hash(
        format!(
            "{GENERATOR}\0{}\0{scope}\0{source_hash}",
            revision.as_deref().unwrap_or("unknown")
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let config_path = workspace.join(".aether/config.toml");
    let config_bytes = match fs::read(&config_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error).context("read pre-flight configuration fingerprint"),
    };
    let terms = keywords(&request.task);
    let mut sections = vec![
        Section { title: "Revision, freshness and coverage", empty: "Freshness unknown.", items: vec![
            format!("HEAD: {}. Scope: {}. Source fingerprint covers {} readable supported source files; {} files excluded/unreadable/over limit. It excludes non-source files and ignored build/store directories.", revision.as_deref().unwrap_or("unavailable"), if scope.is_empty() { "." } else { scope }, sources.len(), excluded),
            "Completed-index and projection generations: unknown. Source reads do not prove SIR or graph freshness. No provider, SurrealDB or vector backend is opened.".to_owned(),
        ] },
        Section { title: "Investigation order", empty: "No lexical source or catalog match; use ordinary repository exploration.", items: Vec::new() },
        Section { title: "Dependencies and reverse impact", empty: "No stored relationships found; this does not establish absence of dependencies.", items: Vec::new() },
        Section { title: "Changes and recorded reasons", empty: "No relevant recorded history or decision reason found; reasons remain unknown.", items: Vec::new() },
        Section { title: "Risks and uncertainty", empty: "No additional observed risks.", items: vec![
            "Structural name matching is heuristic; missing imports, macros, dynamic dispatch and external calls remain possible. Dependencies and dependents are separate directions; neither set is exhaustive. No SIR text is treated as a recorded decision reason.".to_owned(),
        ] },
        Section { title: "Relevant tests", empty: "No related test evidence found. Search and run behavior-specific tests before changing code.", items: Vec::new() },
    ];
    match crate::indexing_gaps::read_summary(&workspace) {
        Ok(Some(summary)) => sections[0].items.push(summary),
        Ok(None) => {}
        Err(error) => sections[0]
            .items
            .push(format!("Indexing quarantine report unavailable: {error:#}")),
    }
    let mut omitted = BTreeMap::new();
    let store_path = workspace.join(".aether/meta.sqlite");
    let store = if store_path.is_file() {
        match SqliteStore::open_readonly(&workspace) {
            Ok(store) => Some(store),
            Err(error) => {
                sections[4].items.push(format!("Catalog unavailable ({error}); using source-only fallback without initialization."));
                None
            }
        }
    } else {
        sections[0].items.push(
            "No index exists: source-only fallback. No .aether directory or database was created."
                .to_owned(),
        );
        None
    };
    let mut selected = Vec::new();
    if let Some(store) = &store {
        let schema = store
            .get_schema_version()
            .context("read pre-flight history capability")?;
        sections[0].items.push(if schema.version >= 22 {
            "Historical retention is available; candidate lineage is heuristic and revision provenance may be unknown.".to_owned()
        } else {
            "Historical retention is unavailable in this older index schema; no retirement or lineage completeness is claimed and no migration was run.".to_owned()
        });
        let cleanup = store.retirement_cleanup_summary(0)?;
        sections[0].items.push(if cleanup.available {
            format!("External retirement cleanup: {} graph components pending, {} vector components pending, {} failed, {} legacy unknown, {} superseded by reintroduction. Superseded components require ordinary projection reconstruction before freshness. This backlog is not a projection freshness proof. Read-only briefing never repairs it.", cleanup.pending_graph, cleanup.pending_vector, cleanup.failed_components, cleanup.legacy_unknown_components, cleanup.superseded_components)
        } else {
            "External retirement cleanup: unknown (this index predates the queue). Read-only briefing never repairs it.".to_owned()
        });
        match store.search_symbols_by_terms(&terms, 100) {
            Ok(candidates) => {
                for candidate in candidates {
                    if !in_scope(&candidate.file_path, scope) {
                        continue;
                    }
                    if selected.len() == 8 {
                        *omitted.entry("investigation_candidates".to_owned()).or_default() += 1;
                        continue;
                    }
                    if let Some(record) = store.get_symbol_record(&candidate.symbol_id)? {
                        sections[1].items.push(format!("{} — ranked task-term match in symbol name/path; source [{}]({}); symbol ID `{}`. Current source/annotation freshness: unverified.", record.qualified_name, record.file_path, record.file_path, record.id));
                        selected.push(record);
                    }
                }
            }
            Err(error) => sections[4].items.push(format!("Catalog search unavailable ({error}); source-only lexical matches remain available.")),
        }
    }
    let mut matched_sources = sources
        .iter()
        .filter_map(|source| {
            let path = source.path.to_lowercase();
            let text = source.text.to_lowercase();
            let score = terms
                .iter()
                .filter(|term| path.contains(term.as_str()) || text.contains(term.as_str()))
                .count();
            (score > 0).then_some((score, source))
        })
        .collect::<Vec<_>>();
    matched_sources.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.path.cmp(&right.1.path))
    });
    let mut paths = selected
        .iter()
        .map(|record| record.file_path.clone())
        .collect::<BTreeSet<_>>();
    if let Some(store) = &store {
        let retirements = store.search_symbol_retirement_summaries(&terms, 100)?;
        let mut included = 0;
        for retired in retirements {
            if !in_scope(&retired.symbol.file_path, scope) {
                continue;
            }
            if included == 8 {
                *omitted.entry("retired_matches".to_owned()).or_default() += 1;
                continue;
            }
            included += 1;
            paths.insert(retired.symbol.file_path.clone());
            sections[1].items.push(format!("Retired declaration `{}` at [{}]({}) — task-term match in retained name/path, not active source; citation `.aether/meta.sqlite#symbol_retirements/{}`. Inspect its path history for removed behavior; no replacement or rename is inferred.", retired.symbol.qualified_name, retired.symbol.file_path, retired.symbol.file_path, retired.retirement_id));
            sections[3].items.push(format!("Retired metadata: `{}` at `{}`, retirement {} at {}, recorded retirement reason `{}`; cite `.aether/meta.sqlite#symbol_retirements/{}`. Retirement metadata alone does not explain the design decision.", retired.symbol_id, retired.symbol.file_path, retired.retirement_id, retired.retired_at, retired.reason, retired.retirement_id));
        }
    }
    for (_, source) in &matched_sources {
        if sections[1].items.len() >= 8 {
            *omitted.entry("source_matches".to_owned()).or_default() += 1;
            continue;
        }
        if paths.insert(source.path.clone()) {
            let line = source
                .text
                .lines()
                .position(|line| terms.iter().any(|term| line.to_lowercase().contains(term)))
                .map_or(1, |line| line + 1);
            sections[1].items.push(format!("[{}:{}]({}:{}): source-text/path task-term match; inspect behavior and callers before editing.", source.path, line, source.path, line));
        }
    }
    if let Some(store) = &store {
        gather_catalog_evidence(
            store,
            &selected,
            &paths,
            &terms,
            &mut sections,
            &mut omitted,
        )?;
    }
    let mut commits = BTreeMap::<String, (aether_core::CommitInfo, BTreeSet<String>)>::new();
    if let Some(git) = &git {
        for path in paths.iter().take(8) {
            let history = match git.file_history(&workspace.join(path), 3) {
                Ok(history) => history,
                Err(error) => {
                    sections[4].items.push(format!("Git path history unavailable for `{path}`: {error}; historical coverage is incomplete."));
                    continue;
                }
            };
            for commit in history {
                commits
                    .entry(commit.hash.clone())
                    .or_insert_with(|| (commit, BTreeSet::new()))
                    .1
                    .insert(path.clone());
            }
        }
        if commits.is_empty() {
            sections[3].items.push("No path-history entries returned. Untracked paths and shallow history can leave relevant recorded reasons unavailable.".to_owned());
        }
        let mut commits = commits.into_values().collect::<Vec<_>>();
        commits.sort_by(|left, right| {
            right
                .0
                .timestamp
                .cmp(&left.0.timestamp)
                .then_with(|| left.0.hash.cmp(&right.0.hash))
        });
        for (commit, paths) in commits {
            let paths = paths
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ");
            sections[3].items.push(format!("git:{} paths {} ({}): recorded commit message: {}. A message is author evidence, not a verified causal explanation.", commit.hash, paths, commit.timestamp, excerpt(&commit.message, 650)));
        }
    } else {
        sections[3].items.push("Git history unavailable; recorded historical reasons cannot be inferred from current code.".to_owned());
    }
    for (_, source) in matched_sources {
        if aether_core::is_test_file(&source.path) && sections[5].items.len() < 12 {
            sections[5].items.push(format!("[{}]({}) — task-term match in test source; relevance is lexical and has not been executed.", source.path, source.path));
        }
    }
    let mut map = PreflightMap {
        generator: GENERATOR.to_owned(),
        hash_algorithm: "BLAKE3".to_owned(),
        revision,
        source_hash,
        worktree_hash,
        config_hash: blake3::hash(&config_bytes).to_hex().to_string(),
        task_hash: blake3::hash(request.task.as_bytes()).to_hex().to_string(),
        map_hash: String::new(),
        context: String::new(),
        omitted_items: omitted,
        estimated_tokens: 0,
        estimate_basis: "ceil(serialized UTF-8 bytes / 4); not a provider tokenizer count"
            .to_owned(),
    };
    loop {
        map.context = render(&request.task, &sections, &map.omitted_items);
        map.map_hash = map_identity(&map)?;
        map.estimated_tokens = 0;
        for _ in 0..8 {
            let estimate = measure(&map)?;
            if estimate <= map.estimated_tokens {
                break;
            }
            map.estimated_tokens = estimate;
        }
        if measure(&map)? > map.estimated_tokens {
            bail!("pre-flight size estimate did not converge");
        }
        if map.estimated_tokens <= request.max_tokens {
            return Ok(map);
        }
        // Preserve every fixed section and its uncertainty statement. Remove low
        // priority evidence first and record every removed item rather than cutting
        // arbitrary bytes through a citation or a serialized JSON string.
        let removable = [3, 2, 5, 1, 4]
            .into_iter()
            .find(|index| sections[*index].items.len() > usize::from(*index == 4));
        let Some(index) = removable else {
            bail!(
                "max_tokens is too small for pre-flight metadata and fixed sections; need at least {} estimated tokens",
                map.estimated_tokens
            );
        };
        sections[index].items.pop();
        *map.omitted_items
            .entry(sections[index].title.to_owned())
            .or_default() += 1;
    }
}

fn gather_catalog_evidence(
    store: &SqliteStore,
    selected: &[SymbolRecord],
    paths: &BTreeSet<String>,
    terms: &[String],
    sections: &mut [Section],
    omitted: &mut BTreeMap<String, usize>,
) -> Result<()> {
    let mut relationships = BTreeSet::new();
    let mut notes = BTreeSet::new();
    let mut tests = BTreeSet::new();
    for record in selected {
        for edge in store.list_symbol_edges_for_source_and_kinds(
            &record.id,
            &[
                EdgeKind::Calls,
                EdgeKind::DependsOn,
                EdgeKind::TypeRef,
                EdgeKind::Implements,
            ],
        )? {
            let candidates = store
                .list_symbols_for_call_target(&edge.target_qualified_name)?
                .into_iter()
                .filter(|candidate| same_language_family(&record.language, &candidate.language))
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                relationships.insert(format!(
                    "Dependency: `{}` → `{}` ({}, unresolved/external unknown); source [{}]({}).",
                    record.qualified_name,
                    edge.target_qualified_name,
                    edge.edge_kind.as_str(),
                    edge.file_path,
                    edge.file_path
                ));
            }
            let label = if candidates.len() > 1 {
                "ambiguous name candidate"
            } else {
                "heuristic name match"
            };
            for candidate in candidates {
                relationships.insert(format!("Dependency: `{}` → `{}` ({}, {}); inspect [{}]({}) because [{}]({}) references that name.", record.qualified_name, candidate.qualified_name, edge.edge_kind.as_str(), label, candidate.file_path, candidate.file_path, edge.file_path, edge.file_path));
            }
        }
        for edge in store.list_symbol_edges_for_target_and_kinds(
            &record.qualified_name,
            &[
                EdgeKind::Calls,
                EdgeKind::DependsOn,
                EdgeKind::TypeRef,
                EdgeKind::Implements,
            ],
        )? {
            if let Some(caller) = store.get_symbol_record(&edge.source_id)?
                && same_language_family(&record.language, &caller.language)
            {
                relationships.insert(format!("Dependent: `{}` ← `{}` ({}, heuristic name match); inspect [{}]({}) for regressions when changing [{}]({}).", record.qualified_name, caller.qualified_name, edge.edge_kind.as_str(), caller.file_path, caller.file_path, record.file_path, record.file_path));
                if aether_core::is_test_symbol(&caller.qualified_name, &caller.file_path) {
                    tests.insert(format!("[{}]({}) `{}` — heuristic reverse `{}` reference to `{}` from an indexed test symbol; scope does not exclude dependent tests. Not executed; the name match is not proof of behavioral coverage.", caller.file_path, caller.file_path, caller.qualified_name, edge.edge_kind.as_str(), record.qualified_name));
                    for intent in store.list_test_intents_for_symbol(&caller.id)? {
                        tests.insert(format!("[{}]({}) `{}` — test intent `{}` on that heuristic dependent: {}. Not executed.", intent.file_path, intent.file_path, intent.test_name, intent.intent_id, excerpt(&intent.intent_text, 220)));
                    }
                }
            }
        }
        for version in store.list_recent_sir_history_metadata(&record.id, 2)? {
            sections[3].items.push(format!("SIR history: `{}` version {} at {} (commit {}, hash {}); citation `.aether/meta.sqlite#sir_history/{}/{}`. This is an annotation observation, not a recorded design reason.", record.qualified_name, version.version, version.created_at, version.commit_hash.as_deref().unwrap_or("unknown"), version.sir_hash, record.id, version.version));
        }
        for lineage in store.list_symbol_lineage(&record.id)? {
            sections[3].items.push(format!("Lineage candidate: `{}` → `{}` ({}; {}); retirement {}. Evidence: {}. This historical ID-associated candidate is not a proven rename or identity equivalence; applicability to the current incarnation is unverified.", lineage.old_symbol_id, lineage.new_symbol_id, lineage.relation, lineage.confidence, lineage.retirement_id, excerpt(&lineage.evidence, 350)));
            if lineage.new_symbol_id == record.id {
                // The lineage row identifies one immutable retirement, not every
                // later incarnation that happens to reuse the old declaration ID.
                // Full archived payloads remain an explicit timeline lookup.
                sections[3].items.push(format!("Historical ID-associated retirement evidence: `.aether/meta.sqlite#symbol_retirements/{}`; symbol `{}`. This cites only the exact retirement recorded by this historical candidate lineage. Applicability to the current incarnation is unverified; other incarnations remain separate. Full retained evidence is available through the symbol timeline.", lineage.retirement_id, lineage.old_symbol_id));
            }
        }
        // Test intent extraction can preserve a production name even when a
        // parser misses the call inside a macro. Query that declaration's name,
        // not the whole task or every test file, and keep the weaker relation explicit.
        let leaf = record
            .qualified_name
            .rsplit("::")
            .next()
            .unwrap_or(&record.qualified_name);
        let leaf = leaf.rsplit('.').next().unwrap_or(leaf).to_lowercase();
        if leaf.len() >= 3 {
            for intent in store.search_test_intents_lexical(&leaf, 8)? {
                if same_language_family(&record.language, &intent.language)
                    && (intent.test_name.to_lowercase().contains(&leaf)
                        || intent.intent_text.to_lowercase().contains(&leaf)
                        || intent.file_path.to_lowercase().contains(&leaf))
                {
                    tests.insert(format!("[{}]({}) `{}` — heuristic declaration-name match for `{}` in stored test intent `{}`: {}. This does not prove a dependency or behavioral coverage. Not executed.", intent.file_path, intent.file_path, intent.test_name, record.qualified_name, intent.intent_id, excerpt(&intent.intent_text, 220)));
                }
            }
        }
        for intent in store.list_test_intents_for_symbol(&record.id)? {
            tests.insert(format!(
                "[{}]({}) `{}` — linked to `{}` by stored test intent `{}`: {}. Not executed.",
                intent.file_path,
                intent.file_path,
                intent.test_name,
                record.qualified_name,
                intent.intent_id,
                excerpt(&intent.intent_text, 220)
            ));
        }
    }
    for path in paths {
        for note in store.list_project_notes_for_file_ref(path, 10)? {
            if !note.is_archived && notes.insert(note.note_id.clone()) {
                sections[3].items.push(format!("Recorded note `{}` ({}, {}, path [{}]({})): {}. Citation `.aether/meta.sqlite#project_notes/{}`; source text is retained without inferring its truth.", note.note_id, note.source_type, note.created_at, path, path, excerpt(&note.content, 500), note.note_id));
            }
        }
    }
    if !terms.is_empty() {
        for note in store.search_project_notes_lexical(&terms.join(" "), 20, false, &[])? {
            if notes.insert(note.note_id.clone()) {
                sections[3].items.push(format!("Recorded note `{}` ({}, {}), task-term lexical match with unverified scope: {}. Citation `.aether/meta.sqlite#project_notes/{}`. No matching structured file reference is established; do not infer authority or causation from this match.", note.note_id, note.source_type, note.created_at, excerpt(&note.content, 500), note.note_id));
            }
        }
    }
    let relationship_count = relationships.len();
    sections[2].items.extend(relationships.into_iter().take(32));
    if relationship_count > 32 {
        omitted.insert("relationships".to_owned(), relationship_count - 32);
    }
    let test_count = tests.len();
    sections[5].items.extend(tests.into_iter().take(12));
    if test_count > 12 {
        omitted.insert("tests".to_owned(), test_count - 12);
    }
    Ok(())
}

fn map_identity(map: &PreflightMap) -> Result<String> {
    // serde_json::Value objects use sorted keys in this workspace, so the hash
    // binds canonical provenance and all delivered evidence without recursion.
    let mut value = serde_json::to_value(map)?;
    let object = value
        .as_object_mut()
        .context("pre-flight map must serialize to an object")?;
    object.remove("map_hash");
    object.remove("estimated_tokens");
    Ok(blake3::hash(&serde_json::to_vec(&value)?)
        .to_hex()
        .to_string())
}

fn hash_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn in_scope(path: &str, scope: &str) -> bool {
    scope.is_empty()
        || path == scope
        || path.starts_with(&format!("{}/", scope.trim_end_matches('/')))
}

fn read_sources(workspace: &Path, scope: &str) -> Result<(Vec<SourceFile>, usize)> {
    let mut sources = Vec::new();
    let mut excluded = 0;
    let mut bytes = 0;
    let mut paths = Vec::new();
    for entry in WalkDir::new(workspace)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_dir()
                || !matches!(
                    entry.file_name().to_str(),
                    Some(
                        ".git"
                            | ".aether"
                            | "target"
                            | "node_modules"
                            | ".venv"
                            | "vendor"
                            | ".codex"
                    )
                )
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                excluded += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() || aether_parse::language_for_path(entry.path()).is_none() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(workspace)
            .context("source path escaped pre-flight workspace")?
            .to_string_lossy()
            .replace('\\', "/");
        if in_scope(&relative, scope) {
            paths.push((relative, entry.path().to_path_buf()));
        }
    }
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    for (path, absolute) in paths {
        if sources.len() >= MAX_FILES || bytes >= MAX_SOURCE_BYTES {
            excluded += 1;
            continue;
        }
        let metadata = match fs::metadata(&absolute) {
            Ok(value) => value,
            Err(_) => {
                excluded += 1;
                continue;
            }
        };
        if metadata.len() > MAX_FILE_BYTES || metadata.len() as usize > MAX_SOURCE_BYTES - bytes {
            excluded += 1;
            continue;
        }
        match fs::read_to_string(&absolute) {
            Ok(text) => {
                bytes += text.len();
                sources.push(SourceFile { path, text });
            }
            Err(_) => excluded += 1,
        }
    }
    Ok((sources, excluded))
}

fn keywords(task: &str) -> Vec<String> {
    task.split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|word| word.len() >= 3)
        .map(str::to_lowercase)
        .filter(|word| {
            !matches!(
                word.as_str(),
                "the"
                    | "and"
                    | "with"
                    | "that"
                    | "this"
                    | "from"
                    | "for"
                    | "add"
                    | "fix"
                    | "change"
                    | "update"
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(16)
        .collect()
}

fn excerpt(value: &str, max_chars: usize) -> String {
    let value = value.replace(['\0', '\u{1b}'], "");
    let mut chars = value.chars();
    let text = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{text} [excerpt; full text at citation]")
    } else {
        text
    }
}

fn render(task: &str, sections: &[Section], omitted: &BTreeMap<String, usize>) -> String {
    let mut output = format!("# Task pre-flight\n\nTask: {}\n", excerpt(task, 240));
    for section in sections {
        output.push_str(&format!("\n## {}\n\n", section.title));
        let trimmed = omitted.get(section.title).copied().unwrap_or(0);
        if trimmed > 0 {
            output.push_str(&format!("{trimmed} evidence items omitted to fit the response budget; omission does not establish absence.\n"));
        } else if section.items.is_empty() {
            output.push_str(section.empty);
            output.push('\n');
        }
        for item in &section.items {
            output.push_str("- ");
            output.push_str(item);
            output.push('\n');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{
        ProjectNoteRecord, SymbolCatalogStore, SymbolRelationStore, TestIntentRecord,
    };
    use std::process::Command;

    fn request(task: &str) -> PreflightRequest {
        PreflightRequest {
            task: task.to_owned(),
            scope: None,
            max_tokens: 8_000,
        }
    }

    fn symbol(id: &str, path: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: id.to_owned(),
            signature_fingerprint: id.to_owned(),
            last_seen_at: 1,
        }
    }

    #[test]
    fn review_regression_budget_omission_is_not_reported_as_absence() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::write(workspace.path().join("cache.rs"), "fn cache() { disk(); }")?;
        let store = SqliteStore::open(workspace.path())?;
        store.upsert_symbol(symbol("cache", "cache.rs"))?;
        store.upsert_symbol(symbol("disk", "disk.rs"))?;
        store.upsert_edges(&[SymbolEdge {
            source_id: "cache".to_owned(),
            target_qualified_name: "disk".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "cache.rs".to_owned(),
        }])?;
        let mut input = request("cache");
        input.max_tokens = 512;
        // A deterministic measurement forces the last relationship off the budget.
        let map = assemble_preflight_measured(workspace.path(), &input, |map| {
            Ok(
                if map
                    .omitted_items
                    .contains_key("Dependencies and reverse impact")
                {
                    1
                } else {
                    513
                },
            )
        })?;
        assert!(
            !map.context.contains("No stored relationships found"),
            "{}",
            map.context
        );
        assert!(map.context.contains("omitted to fit"), "{}", map.context);
        Ok(())
    }

    #[test]
    fn missing_index_returns_task_map_without_creating_store_or_needing_branch() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::write(workspace.path().join("cache.rs"), "fn cache() {}")?;
        let map = assemble_preflight(workspace.path(), &request("repair cache"))?;
        assert!(map.context.contains("cache.rs:1"));
        assert!(
            map.context
                .contains("No index exists: source-only fallback")
        );
        assert!(map.context.contains("Changes and recorded reasons"));
        assert!(map.context.contains("Git history unavailable"));
        assert!(!workspace.path().join(".aether").exists());
        assert!(map.revision.is_none());
        for _ in 0..5 {
            assert_eq!(
                assemble_preflight(workspace.path(), &request("repair cache"))?,
                map
            );
        }
        fs::write(workspace.path().join("unrelated.rs"), "fn unrelated() {}")?;
        let changed = assemble_preflight(workspace.path(), &request("repair cache"))?;
        assert_ne!(changed.source_hash, map.source_hash);
        assert_ne!(changed.worktree_hash, map.worktree_hash);
        assert_ne!(changed.map_hash, map.map_hash);
        fs::create_dir(workspace.path().join(".aether"))?;
        fs::write(
            workspace.path().join(".aether/config.toml"),
            "# configuration identity changed\n",
        )?;
        let configured = assemble_preflight(workspace.path(), &request("repair cache"))?;
        assert_eq!(configured.source_hash, changed.source_hash);
        assert_eq!(configured.context, changed.context);
        assert_ne!(configured.config_hash, changed.config_hash);
        assert_ne!(configured.map_hash, changed.map_hash);
        Ok(())
    }

    #[test]
    fn single_task_map_includes_reverse_impact_recorded_reasons_and_linked_tests() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir(workspace.path().join("src"))?;
        fs::create_dir(workspace.path().join("tests"))?;
        fs::write(
            workspace.path().join("src/cache.rs"),
            "fn cache() { disk(); }",
        )?;
        fs::write(
            workspace.path().join("src/client.rs"),
            "fn client() { cache(); }",
        )?;
        fs::write(workspace.path().join("src/disk.rs"), "fn disk() {}")?;
        fs::write(workspace.path().join("src/schema.rs"), "struct Schema;")?;
        let store = SqliteStore::open(workspace.path())?;
        for record in [
            symbol("cache", "src/cache.rs"),
            symbol("client", "src/client.rs"),
            symbol("disk", "src/disk.rs"),
            symbol("schema", "src/schema.rs"),
        ] {
            store.upsert_symbol(record)?;
        }
        store.upsert_edges(&[
            SymbolEdge {
                source_id: "cache".to_owned(),
                target_qualified_name: "disk".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/cache.rs".to_owned(),
            },
            SymbolEdge {
                source_id: "client".to_owned(),
                target_qualified_name: "cache".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/client.rs".to_owned(),
            },
        ])?;
        store.upsert_edges(&[SymbolEdge {
            source_id: "cache".to_owned(),
            target_qualified_name: "schema".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/cache.rs".to_owned(),
        }])?;
        for edge_kind in [EdgeKind::DependsOn, EdgeKind::TypeRef, EdgeKind::Implements] {
            store.upsert_edges(&[
                SymbolEdge {
                    source_id: "cache".to_owned(),
                    target_qualified_name: "disk".to_owned(),
                    edge_kind,
                    file_path: "src/cache.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: "client".to_owned(),
                    target_qualified_name: "cache".to_owned(),
                    edge_kind,
                    file_path: "src/client.rs".to_owned(),
                },
            ])?;
        }
        store.upsert_project_note(ProjectNoteRecord {
            note_id: "decision-full-id".to_owned(),
            content: "Keep cache eviction explicit because disk replay depends on ordering."
                .to_owned(),
            content_hash: "note".to_owned(),
            source_type: "human".to_owned(),
            source_agent: None,
            tags: Vec::new(),
            entity_refs: Vec::new(),
            file_refs: vec!["src/cache.rs".to_owned()],
            symbol_refs: Vec::new(),
            created_at: 1,
            updated_at: 1,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        })?;
        store.replace_test_intents_for_file(
            "tests/regression.rs",
            &[TestIntentRecord {
                intent_id: "eviction-order".to_owned(),
                file_path: "tests/regression.rs".to_owned(),
                test_name: "keeps_eviction_order".to_owned(),
                intent_text: "preserve replay ordering".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: Some("cache".to_owned()),
                created_at: 1,
                updated_at: 1,
            }],
        )?;
        let mut input = request("repair cache");
        input.scope = Some("src/cache.rs".to_owned());
        let map = assemble_preflight(workspace.path(), &input)?;
        assert!(
            map.context.contains("Dependency: `cache` → `disk`"),
            "{}",
            map.context
        );
        assert!(
            map.context
                .contains("Dependency: `cache` → `schema` (type_ref"),
            "{}",
            map.context
        );
        assert!(
            map.context.contains("Dependent: `cache` ← `client`"),
            "{}",
            map.context
        );
        for kind in ["calls", "depends_on", "type_ref", "implements"] {
            assert!(
                map.context
                    .contains(&format!("Dependency: `cache` → `disk` ({kind},")),
                "{}",
                map.context
            );
            assert!(
                map.context
                    .contains(&format!("Dependent: `cache` ← `client` ({kind},")),
                "{}",
                map.context
            );
        }
        assert!(map.context.contains("src/disk.rs"));
        assert!(map.context.contains("src/client.rs"));
        assert!(map.context.contains("decision-full-id"));
        assert!(
            map.context
                .contains("because disk replay depends on ordering")
        );
        assert!(map.context.contains("tests/regression.rs"));
        assert!(map.context.contains("eviction-order"));
        assert!(map.context.contains("heuristic name match"));
        assert!(map.context.contains("freshness: unverified"));
        assert_eq!(
            store
                .get_project_note("decision-full-id")?
                .map(|note| note.access_count),
            Some(0)
        );
        assert!(!workspace.path().join(".aether/graph").exists());
        Ok(())
    }

    #[test]
    fn lineage_citation_does_not_attach_later_same_id_incarnations() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("src/cache.rs"),
            "fn cache(value: i64) {}",
        )?;
        let store = SqliteStore::open(workspace.path())?;
        let mut old = symbol("A", "src/cache.rs");
        old.qualified_name = "cache".to_owned();
        let mut current = symbol("B", "src/cache.rs");
        current.qualified_name = "cache".to_owned();
        store.upsert_symbol(old.clone())?;
        store.upsert_symbol(current.clone())?;
        store.reconcile_and_prune(&[("A".to_owned(), "B".to_owned())], &[])?;
        let lineage = store
            .list_symbol_lineage("B")?
            .into_iter()
            .find(|lineage| lineage.new_symbol_id == "B")
            .ok_or_else(|| anyhow::anyhow!("missing A-to-B lineage fixture"))?;
        // Same declaration ID, independently reintroduced and retired later.
        store.upsert_symbol(old)?;
        store.mark_removed("A")?;
        let later = store
            .list_symbol_retirements("A")?
            .into_iter()
            .find(|retired| retired.retirement_id != lineage.retirement_id)
            .ok_or_else(|| anyhow::anyhow!("missing later A retirement fixture"))?;
        for reintroduce_destination in [false, true] {
            if reintroduce_destination {
                // The destination ID can also refer to a later incarnation. A
                // historical candidate must not be asserted as its predecessor.
                store.mark_removed("B")?;
                store.upsert_symbol(current.clone())?;
            }
            let map = assemble_preflight(workspace.path(), &request("inspect cache"))?;
            let candidates = map
                .context
                .lines()
                .filter(|line| line.starts_with("- Lineage candidate:"))
                .collect::<Vec<_>>();
            assert_eq!(candidates.len(), 1, "{}", map.context);
            assert!(
                candidates[0].contains("applicability to the current incarnation is unverified"),
                "{}",
                map.context
            );
            let predecessors = map
                .context
                .lines()
                .filter(|line| line.starts_with("- Historical ID-associated retirement evidence:"))
                .collect::<Vec<_>>();
            assert_eq!(predecessors.len(), 1, "{}", map.context);
            assert!(
                predecessors[0]
                    .contains(&format!("#symbol_retirements/{}`", lineage.retirement_id)),
                "{}",
                map.context
            );
            assert!(
                !predecessors[0].contains(&format!("#symbol_retirements/{}`", later.retirement_id)),
                "{}",
                map.context
            );
            assert!(
                predecessors[0].contains("Applicability to the current incarnation is unverified"),
                "{}",
                map.context
            );
        }
        Ok(())
    }

    #[test]
    fn scoped_map_keeps_outside_scope_tests_and_unscoped_recorded_notes() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir(workspace.path().join("src"))?;
        fs::create_dir(workspace.path().join("tests"))?;
        fs::write(workspace.path().join("src/cache.rs"), "fn cache() {}")?;
        fs::write(
            workspace.path().join("tests/cache_test.rs"),
            "fn test_cache() { cache(); }",
        )?;
        let store = SqliteStore::open(workspace.path())?;
        store.upsert_symbol(symbol("cache", "src/cache.rs"))?;
        store.upsert_symbol(symbol("test_cache", "tests/cache_test.rs"))?;
        store.upsert_edges(&[SymbolEdge {
            source_id: "test_cache".to_owned(),
            target_qualified_name: "cache".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "tests/cache_test.rs".to_owned(),
        }])?;
        store.replace_test_intents_for_file(
            "tests/macro.rs",
            &[TestIntentRecord {
                intent_id: "macro-cache".to_owned(),
                file_path: "tests/macro.rs".to_owned(),
                test_name: "cache_retries_are_bounded".to_owned(),
                intent_text: "assert_eq!(cache(4), 4)".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: None,
                created_at: 1,
                updated_at: 1,
            }],
        )?;
        store.upsert_project_note(ProjectNoteRecord {
            note_id: "unscoped-decision-id".to_owned(),
            content: "CACHE-RETRY: bound cache retries to prevent duplicate charges.".to_owned(),
            content_hash: "unscoped".to_owned(),
            source_type: "human".to_owned(),
            source_agent: None,
            tags: Vec::new(),
            entity_refs: Vec::new(),
            file_refs: Vec::new(),
            symbol_refs: Vec::new(),
            created_at: 1,
            updated_at: 1,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        })?;
        let mut input = request("cache retry policy");
        input.scope = Some("src".to_owned());
        let map = assemble_preflight(workspace.path(), &input)?;
        let tests = map
            .context
            .split("## Relevant tests")
            .nth(1)
            .unwrap_or_default();
        assert!(tests.contains("tests/cache_test.rs"), "{}", map.context);
        assert!(tests.contains("heuristic reverse `calls`"));
        assert!(tests.contains("tests/macro.rs"));
        assert!(tests.contains("heuristic declaration-name match"));
        assert!(map.context.contains("unscoped-decision-id"));
        assert!(map.context.contains("duplicate charges"));
        assert!(map.context.contains("unverified scope"));
        assert_eq!(
            store
                .get_project_note("unscoped-decision-id")?
                .map(|note| note.access_count),
            Some(0)
        );
        Ok(())
    }

    #[test]
    fn retired_name_and_deleted_path_reach_recorded_git_history_without_inventing_lineage()
    -> Result<()> {
        let workspace = tempfile::tempdir()?;
        fs::create_dir(workspace.path().join("src"))?;
        git(workspace.path(), &["init", "--quiet"])?;
        fs::write(
            workspace.path().join("src/old_bridge.rs"),
            "fn legacy_bridge() {}",
        )?;
        git(workspace.path(), &["add", "src/old_bridge.rs"])?;
        git(
            workspace.path(),
            &["commit", "--quiet", "-m", "Add old bridge"],
        )?;
        let store = SqliteStore::open(workspace.path())?;
        store.upsert_symbol(symbol("legacy_bridge", "src/old_bridge.rs"))?;
        fs::remove_file(workspace.path().join("src/old_bridge.rs"))?;
        git(workspace.path(), &["add", "src/old_bridge.rs"])?;
        git(
            workspace.path(),
            &[
                "commit",
                "--quiet",
                "-m",
                "Delete bridge\n\nDELETED-BRIDGE: legacy bridge bypassed the validation boundary.",
            ],
        )?;
        store.mark_removed("legacy_bridge")?;
        let mut input = request("why was legacy_bridge deleted");
        input.scope = Some("src".to_owned());
        let map = assemble_preflight(workspace.path(), &input)?;
        assert!(
            map.context.contains("Retired declaration `legacy_bridge`"),
            "{}",
            map.context
        );
        assert!(map.context.contains("src/old_bridge.rs"));
        assert!(
            map.context
                .contains("DELETED-BRIDGE: legacy bridge bypassed")
        );
        assert!(map.context.contains("no replacement or rename is inferred"));
        assert!(map.context.contains("symbol_retirements/"));
        Ok(())
    }

    fn git(workspace: &Path, args: &[&str]) -> Result<()> {
        let result = Command::new("git")
            .current_dir(workspace)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .context("run disposable history fixture git")?;
        if !result.status.success() {
            bail!(
                "fixture git failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Ok(())
    }

    #[test]
    fn map_renders_shared_commit_reason_once_with_all_related_paths() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        git(workspace.path(), &["init", "--quiet"])?;
        fs::write(workspace.path().join("cache.rs"), "fn cache() {}\n")?;
        fs::write(
            workspace.path().join("cache_client.rs"),
            "fn cache_client() {}\n",
        )?;
        git(workspace.path(), &["add", "."])?;
        git(
            workspace.path(),
            &[
                "commit",
                "--quiet",
                "-m",
                "SHARED-REASON: keep cache and client retry policy consistent.",
            ],
        )?;
        let map = assemble_preflight(workspace.path(), &request("cache retry"))?;
        assert_eq!(
            map.context.matches("SHARED-REASON").count(),
            1,
            "{}",
            map.context
        );
        let history = map
            .context
            .split("## Changes and recorded reasons")
            .nth(1)
            .and_then(|section| section.split("## Risks").next())
            .unwrap_or_default();
        assert!(
            history.contains("cache.rs") && history.contains("cache_client.rs"),
            "{history}"
        );
        Ok(())
    }

    #[test]
    fn map_uses_path_history_including_removed_guard_and_full_reason() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        git(workspace.path(), &["init", "--quiet"])?;
        fs::write(
            workspace.path().join("cache.rs"),
            "fn cache() { guard(); }\n",
        )?;
        git(workspace.path(), &["add", "cache.rs"])?;
        git(
            workspace.path(),
            &[
                "commit",
                "--quiet",
                "-m",
                "Introduce cache guard\n\nRecorded reason: retries must preserve replay order.",
            ],
        )?;
        fs::write(workspace.path().join("cache.rs"), "fn cache() {}\n")?;
        git(workspace.path(), &["add", "cache.rs"])?;
        git(
            workspace.path(),
            &[
                "commit",
                "--quiet",
                "-m",
                "Remove cache guard\n\nRecorded reason: caller now owns retry policy.",
            ],
        )?;
        let map = assemble_preflight(workspace.path(), &request("repair cache retries"))?;
        assert!(map.context.contains("Introduce cache guard"));
        assert!(map.context.contains("retries must preserve replay order"));
        assert!(map.context.contains("caller now owns retry policy"));
        assert!(map.context.contains("git:"));
        Ok(())
    }

    #[test]
    fn map_budget_preserves_fixed_sections_and_reports_omissions() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        for index in 0..20 {
            fs::write(
                workspace.path().join(format!("cache{index}.rs")),
                "fn cache() {}",
            )?;
        }
        let mut input = request("repair cache");
        input.max_tokens = 900;
        let map = assemble_preflight(workspace.path(), &input)?;
        assert!(serde_json::to_vec(&map)?.len().div_ceil(4) <= 900);
        assert!(map.estimated_tokens <= 900);
        assert!(
            map.context
                .contains("Completed-index and projection generations: unknown")
        );
        assert!(
            map.context
                .contains("Source reads do not prove SIR or graph freshness")
        );
        assert!(!map.omitted_items.is_empty());
        for title in [
            "Revision, freshness and coverage",
            "Investigation order",
            "Dependencies and reverse impact",
            "Changes and recorded reasons",
            "Risks and uncertainty",
            "Relevant tests",
        ] {
            assert!(map.context.contains(title));
        }
        assert!(
            assemble_preflight(
                workspace.path(),
                &PreflightRequest {
                    scope: Some("../outside".to_owned()),
                    ..input
                }
            )
            .is_err()
        );
        Ok(())
    }
}
