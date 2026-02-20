use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use aether_analysis::{IntentAnalyzer, IntentSnapshotRequest, VerifyIntentRequest};
use aether_config::load_workspace_config;
use aether_core::{
    Language, Position, SourceRange, Symbol, SymbolChangeEvent, SymbolKind, diff_symbols,
    normalize_path,
};
use aether_infer::ProviderOverrides;
use aether_parse::{SymbolExtractor, language_for_path};
use aether_store::{SqliteStore, Store, SymbolRecord};
use anyhow::{Context, Result, anyhow};

use crate::cli::{SnapshotIntentArgs, VerifyIntentArgs};
use crate::sir_pipeline::{DEFAULT_SIR_CONCURRENCY, SirPipeline};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRegenReport {
    pub snapshot_id: String,
    pub baseline_commit: Option<String>,
    pub head_commit: Option<String>,
    pub changed_files: Vec<String>,
    pub regenerated_symbols: u32,
    pub skipped_files: Vec<String>,
}

pub fn run_snapshot_intent_command(workspace: &Path, args: SnapshotIntentArgs) -> Result<()> {
    let analyzer =
        IntentAnalyzer::new(workspace).context("failed to initialize intent analyzer")?;
    let result = analyzer
        .snapshot_intent(IntentSnapshotRequest {
            scope: args.scope,
            target: args.target,
            label: args.label,
        })
        .context("snapshot-intent failed")?;
    let value =
        serde_json::to_value(result).context("failed to serialize snapshot-intent output")?;
    write_json_to_stdout(&value)
}

pub fn run_verify_intent_command(workspace: &Path, args: VerifyIntentArgs) -> Result<()> {
    let analyzer =
        IntentAnalyzer::new(workspace).context("failed to initialize intent analyzer")?;
    let regenerate = if args.regenerate_sir {
        true
    } else if args.no_regenerate_sir {
        false
    } else {
        analyzer.config().auto_regenerate_sir
    };

    let regen_report =
        regenerate_snapshot_symbols_if_enabled(workspace, &args.snapshot_id, regenerate)
            .context("failed to process regenerate-sir flow")?;

    let mut result = analyzer
        .verify_intent(VerifyIntentRequest {
            snapshot_id: args.snapshot_id,
        })
        .context("verify-intent failed")?;

    if let Some(report) = regen_report {
        result.notes.push(format!(
            "regenerated {} symbols across {} changed files (baseline={}, head={})",
            report.regenerated_symbols,
            report.changed_files.len(),
            report.baseline_commit.as_deref().unwrap_or("unknown"),
            report.head_commit.as_deref().unwrap_or("unknown")
        ));
    }

    let value = serde_json::to_value(result).context("failed to serialize verify-intent output")?;
    write_json_to_stdout(&value)
}

pub fn regenerate_snapshot_symbols_if_enabled(
    workspace: &Path,
    snapshot_id: &str,
    regenerate: bool,
) -> Result<Option<IntentRegenReport>> {
    if !regenerate {
        return Ok(None);
    }

    let report = regenerate_changed_snapshot_symbols(workspace, snapshot_id)?;
    Ok(Some(report))
}

pub fn regenerate_changed_snapshot_symbols(
    workspace: &Path,
    snapshot_id: &str,
) -> Result<IntentRegenReport> {
    let snapshot_id = snapshot_id.trim().to_ascii_lowercase();
    if snapshot_id.is_empty() {
        return Err(anyhow!("snapshot_id is required to regenerate SIR"));
    }

    let store = SqliteStore::open(workspace).context("failed to open store")?;
    let snapshot = store
        .get_intent_snapshot(snapshot_id.as_str())?
        .ok_or_else(|| anyhow!("snapshot '{snapshot_id}' not found"))?;

    let head_commit = resolve_head_commit_hash(workspace);
    let baseline_commit = snapshot.commit_hash.clone();

    let changed_files = list_changed_files(
        workspace,
        baseline_commit.as_deref(),
        head_commit.as_deref(),
    )?;
    if changed_files.is_empty() {
        return Ok(IntentRegenReport {
            snapshot_id,
            baseline_commit,
            head_commit,
            changed_files: Vec::new(),
            regenerated_symbols: 0,
            skipped_files: Vec::new(),
        });
    }

    let config = load_workspace_config(workspace).context("failed to load workspace config")?;
    let mut extractor = SymbolExtractor::new().map_err(|err| anyhow!(err.to_string()))?;
    let pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        DEFAULT_SIR_CONCURRENCY,
        ProviderOverrides {
            provider: Some(config.inference.provider),
            model: config.inference.model,
            endpoint: config.inference.endpoint,
            api_key_env: Some(config.inference.api_key_env),
        },
    )
    .context("failed to initialize SIR pipeline")?;

    let mut regenerated_symbols = 0u32;
    let mut skipped_files = Vec::new();
    let mut stdout = std::io::sink();

    for file_path in &changed_files {
        let normalized = normalize_path(file_path);
        if normalized.is_empty() {
            continue;
        }

        let previous_records = store
            .list_symbols_for_file(normalized.as_str())
            .with_context(|| format!("failed to load symbols for {normalized}"))?;

        let absolute_path = workspace.join(normalized.as_str());
        let language = language_for_path(&absolute_path).or_else(|| {
            previous_records
                .first()
                .map(|record| language_from_str(record.language.as_str()))
        });

        if absolute_path.exists() {
            let Some(language) = language else {
                skipped_files.push(normalized);
                continue;
            };

            let source = fs::read_to_string(&absolute_path).with_context(|| {
                format!("failed to read changed file {}", absolute_path.display())
            })?;
            let current_symbols = extractor
                .extract_from_source(language, normalized.as_str(), &source)
                .map_err(|err| anyhow!(err.to_string()))?;
            let previous_symbols = previous_records
                .iter()
                .map(|record| placeholder_symbol(record, language))
                .collect::<Vec<_>>();
            let event = diff_symbols(
                normalized.as_str(),
                language,
                previous_symbols.as_slice(),
                current_symbols.as_slice(),
            );
            if event.is_empty() {
                continue;
            }
            regenerated_symbols = regenerated_symbols
                .saturating_add((event.added.len() + event.updated.len()) as u32);
            pipeline
                .process_event(&store, &event, false, &mut stdout)
                .with_context(|| format!("failed to process regeneration for {normalized}"))?;
            continue;
        }

        if previous_records.is_empty() {
            continue;
        }

        let language = language.unwrap_or(Language::Rust);
        let removed = previous_records
            .iter()
            .map(|record| placeholder_symbol(record, language))
            .collect::<Vec<_>>();
        let event = SymbolChangeEvent {
            file_path: normalized,
            language,
            added: Vec::new(),
            removed,
            updated: Vec::new(),
        };
        pipeline
            .process_event(&store, &event, false, &mut stdout)
            .context("failed to process regeneration for removed file")?;
    }

    Ok(IntentRegenReport {
        snapshot_id,
        baseline_commit,
        head_commit,
        changed_files,
        regenerated_symbols,
        skipped_files,
    })
}

fn placeholder_symbol(record: &SymbolRecord, language: Language) -> Symbol {
    Symbol {
        id: record.id.clone(),
        language,
        file_path: record.file_path.clone(),
        kind: symbol_kind_from_str(record.kind.as_str()),
        name: symbol_leaf_name(record.qualified_name.as_str()),
        qualified_name: record.qualified_name.clone(),
        signature_fingerprint: record.signature_fingerprint.clone(),
        content_hash: String::new(),
        range: SourceRange {
            start: Position { line: 1, column: 1 },
            end: Position { line: 1, column: 1 },
        },
    }
}

fn list_changed_files(
    workspace: &Path,
    baseline_commit: Option<&str>,
    head_commit: Option<&str>,
) -> Result<Vec<String>> {
    let Some(baseline_commit) = baseline_commit
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let Some(head_commit) = head_commit.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };

    if baseline_commit == head_commit {
        return Ok(Vec::new());
    }

    let range = format!("{baseline_commit}..{head_commit}");
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff")
        .arg("--name-only")
        .arg(range)
        .output()
        .context("failed to run git diff for intent regeneration")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git diff failed: {}",
            String::from_utf8_lossy(output.stderr.as_slice())
        ));
    }

    let mut files = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| String::from_utf8(line.to_vec()).ok())
        .map(|line| normalize_path(line.trim()))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

fn resolve_head_commit_hash(workspace: &Path) -> Option<String> {
    let repo = gix::discover(workspace).ok()?;
    let head = repo.head_id().ok()?.detach();
    Some(head.to_string().to_ascii_lowercase())
}

fn symbol_leaf_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(qualified_name)
        .to_owned()
}

fn symbol_kind_from_str(kind: &str) -> SymbolKind {
    match kind.trim() {
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

fn language_from_str(language: &str) -> Language {
    match language.trim() {
        "rust" => Language::Rust,
        "typescript" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "javascript" => Language::JavaScript,
        "jsx" => Language::Jsx,
        "python" => Language::Python,
        _ => Language::Rust,
    }
}

fn write_json_to_stdout(value: &serde_json::Value) -> Result<()> {
    let mut out = std::io::stdout();
    serde_json::to_writer_pretty(&mut out, value).context("failed to serialize JSON output")?;
    writeln!(&mut out).context("failed to write trailing newline")?;
    Ok(())
}
