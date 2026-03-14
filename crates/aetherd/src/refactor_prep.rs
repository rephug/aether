use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use aether_analysis::{
    PreparedRefactorCandidate, RefactorPreparationRequest, RefactorScope, collect_intent_snapshot,
    prepare_refactor_prep,
};
use aether_config::{AetherConfig, InferenceProviderKind};
use aether_infer::ProviderOverrides;
use aether_store::{SirStateStore, SnapshotStore, SqliteStore};
use anyhow::{Context, Result};
use serde::Serialize;

use crate::cli::{RefactorPrepArgs, RefactorPrepOutputFormat};
use crate::sir_pipeline::{QualityBatchItem, SIR_GENERATION_PASS_DEEP, SirPipeline};

#[derive(Debug)]
pub struct RefactorPrepExecution {
    pub summary: RefactorPrepSummary,
    pub rendered: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefactorPrepSummary {
    pub snapshot_id: String,
    pub scope: String,
    pub total_in_scope_symbols: usize,
    pub selected_count: usize,
    pub deep_requested: usize,
    pub deep_completed: usize,
    pub deep_failed: usize,
    pub deep_failed_symbol_ids: Vec<String>,
    pub forced_cycle_members: usize,
    pub skipped_fresh: usize,
    pub notes: Vec<String>,
    pub candidates: Vec<RefactorPrepCandidateSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefactorPrepCandidateSummary {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub refactor_risk: f64,
    pub risk_factors: Vec<String>,
    pub needs_deep_scan: bool,
    pub deep_scan_completed: bool,
    pub in_cycle: bool,
    pub generation_pass: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct DeepScanOutcome {
    requested: usize,
    succeeded_ids: HashSet<String>,
    failed_symbol_ids: Vec<String>,
}

pub fn run_refactor_prep_command(
    workspace: &Path,
    config: &AetherConfig,
    args: RefactorPrepArgs,
) -> Result<()> {
    let execution = execute_refactor_prep_command(workspace, config, args)?;
    write_rendered_output(execution.rendered.as_str(), "refactor-prep")?;
    if execution.exit_code != 0 {
        std::process::exit(execution.exit_code);
    }
    Ok(())
}

pub fn execute_refactor_prep_command(
    workspace: &Path,
    config: &AetherConfig,
    args: RefactorPrepArgs,
) -> Result<RefactorPrepExecution> {
    execute_refactor_prep_with_executor(workspace, config, args, |store, deep_candidates, local| {
        run_deep_scan_with_pipeline(workspace, config, store, deep_candidates, local)
    })
}

fn execute_refactor_prep_with_executor<F>(
    workspace: &Path,
    config: &AetherConfig,
    args: RefactorPrepArgs,
    executor: F,
) -> Result<RefactorPrepExecution>
where
    F: Fn(&SqliteStore, &[PreparedRefactorCandidate], bool) -> Result<DeepScanOutcome>,
{
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let scope = refactor_scope_from_args(&args)?;
    let prep = prepare_refactor_prep(
        workspace,
        &store,
        RefactorPreparationRequest {
            scope: scope.clone(),
            top_n: args.top_n,
        },
    )
    .context("failed to prepare refactor candidates")?;

    let deep_candidates = prep
        .candidates
        .iter()
        .filter(|candidate| candidate.needs_deep_scan)
        .cloned()
        .collect::<Vec<_>>();
    let deep_outcome = executor(&store, deep_candidates.as_slice(), args.local)?;
    let snapshot = collect_intent_snapshot(
        workspace,
        &store,
        &prep.scope,
        prep.scope_symbols.as_slice(),
        &deep_outcome.succeeded_ids,
    )
    .context("failed to collect refactor intent snapshot")?;
    store
        .create_snapshot(&snapshot)
        .context("failed to persist refactor intent snapshot")?;

    let mut notes = prep.notes;
    if !deep_outcome.failed_symbol_ids.is_empty() {
        notes.push(format!(
            "{} deep scans did not complete successfully.",
            deep_outcome.failed_symbol_ids.len()
        ));
    }
    notes.push(
        "Inference cost tracking is unavailable in the current provider abstraction; counts are reported instead.".to_owned(),
    );

    let candidate_summaries = prep
        .candidates
        .iter()
        .map(|candidate| RefactorPrepCandidateSummary {
            symbol_id: candidate.symbol.id.clone(),
            qualified_name: candidate.symbol.qualified_name.clone(),
            file_path: candidate.symbol.file_path.clone(),
            refactor_risk: candidate.refactor_risk,
            risk_factors: candidate.risk_factors.clone(),
            needs_deep_scan: candidate.needs_deep_scan,
            deep_scan_completed: deep_outcome
                .succeeded_ids
                .contains(candidate.symbol.id.as_str()),
            in_cycle: candidate.in_cycle,
            generation_pass: candidate.current_generation_pass.clone(),
        })
        .collect::<Vec<_>>();
    let summary = RefactorPrepSummary {
        snapshot_id: snapshot.snapshot_id.clone(),
        scope: prep.scope.label(),
        total_in_scope_symbols: prep.scope_symbols.len(),
        selected_count: candidate_summaries.len(),
        deep_requested: deep_outcome.requested,
        deep_completed: deep_outcome.succeeded_ids.len(),
        deep_failed: deep_outcome.failed_symbol_ids.len(),
        deep_failed_symbol_ids: deep_outcome.failed_symbol_ids.clone(),
        forced_cycle_members: prep.forced_cycle_members,
        skipped_fresh: prep.skipped_fresh,
        notes,
        candidates: candidate_summaries,
    };
    let rendered = render_refactor_prep_output(&summary, args.output)?;
    let exit_code = if summary.deep_failed > 0 { 1 } else { 0 };

    let _ = config;

    Ok(RefactorPrepExecution {
        summary,
        rendered,
        exit_code,
    })
}

fn refactor_scope_from_args(args: &RefactorPrepArgs) -> Result<RefactorScope> {
    if let Some(file) = args
        .file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(RefactorScope::File {
            path: file.to_owned(),
        });
    }
    if let Some(crate_name) = args
        .crate_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(RefactorScope::Crate {
            name: crate_name.to_owned(),
        });
    }
    anyhow::bail!("either --file or --crate must be provided")
}

fn run_deep_scan_with_pipeline(
    workspace: &Path,
    config: &AetherConfig,
    store: &SqliteStore,
    candidates: &[PreparedRefactorCandidate],
    local: bool,
) -> Result<DeepScanOutcome> {
    if candidates.is_empty() {
        return Ok(DeepScanOutcome::default());
    }

    let pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        config.sir_quality.deep_concurrency.max(1),
        if local {
            ProviderOverrides {
                provider: Some(InferenceProviderKind::Qwen3Local),
                ..ProviderOverrides::default()
            }
        } else {
            ProviderOverrides {
                provider: config
                    .sir_quality
                    .deep_provider
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        value.parse::<InferenceProviderKind>().map_err(|err| {
                            anyhow::anyhow!(
                                "invalid sir_quality.deep_provider '{}': {}",
                                value,
                                err
                            )
                        })
                    })
                    .transpose()?,
                model: config.sir_quality.deep_model.clone(),
                endpoint: config.sir_quality.deep_endpoint.clone(),
                api_key_env: config.sir_quality.deep_api_key_env.clone(),
            }
        },
    )
    .map(|pipeline| pipeline.with_inference_timeout_secs(config.sir_quality.deep_timeout_secs))
    .context("failed to initialize deep-pass pipeline")?;
    let use_cot = pipeline.provider_name() == InferenceProviderKind::Qwen3Local.as_str();
    let items = candidates
        .iter()
        .map(|candidate| QualityBatchItem {
            symbol: candidate.symbol.clone(),
            priority_score: candidate.refactor_risk,
            enrichment: candidate.enrichment.clone(),
            use_cot,
        })
        .collect::<Vec<_>>();

    let mut sink = std::io::sink();
    pipeline
        .process_quality_batch(store, items, SIR_GENERATION_PASS_DEEP, false, &mut sink)
        .context("deep-pass batch execution failed")?;

    let mut succeeded_ids = HashSet::new();
    let mut failed_symbol_ids = Vec::new();
    for candidate in candidates {
        let completed = store
            .get_sir_meta(candidate.symbol.id.as_str())
            .context("failed to read SIR metadata after deep pass")?
            .is_some_and(|meta| meta.generation_pass.trim().eq_ignore_ascii_case("deep"));
        if completed {
            succeeded_ids.insert(candidate.symbol.id.clone());
        } else {
            failed_symbol_ids.push(candidate.symbol.id.clone());
        }
    }

    Ok(DeepScanOutcome {
        requested: candidates.len(),
        succeeded_ids,
        failed_symbol_ids,
    })
}

fn render_refactor_prep_output(
    summary: &RefactorPrepSummary,
    format: RefactorPrepOutputFormat,
) -> Result<String> {
    match format {
        RefactorPrepOutputFormat::Human => Ok(render_refactor_prep_human(summary)),
        RefactorPrepOutputFormat::Json => {
            serde_json::to_string_pretty(summary).context("failed to serialize refactor-prep JSON")
        }
    }
}

fn render_refactor_prep_human(summary: &RefactorPrepSummary) -> String {
    let mut lines = vec![
        format!("Snapshot: {}", summary.snapshot_id),
        format!("Scope: {}", summary.scope),
        format!("In-scope symbols: {}", summary.total_in_scope_symbols),
        format!(
            "Selected: {} (cycle-forced: {}, already fresh: {})",
            summary.selected_count, summary.forced_cycle_members, summary.skipped_fresh
        ),
        format!(
            "Deep pass: {} requested, {} completed, {} failed",
            summary.deep_requested, summary.deep_completed, summary.deep_failed
        ),
    ];

    if !summary.candidates.is_empty() {
        lines.push(String::new());
        lines.push("Candidates:".to_owned());
        for candidate in &summary.candidates {
            let deep_state = if candidate.needs_deep_scan {
                if candidate.deep_scan_completed {
                    "deep_scanned"
                } else {
                    "deep_pending_or_failed"
                }
            } else {
                "fresh_deep_sir"
            };
            lines.push(format!(
                "- {} [{}] risk {:.2} {}",
                candidate.qualified_name, candidate.file_path, candidate.refactor_risk, deep_state
            ));
            if !candidate.risk_factors.is_empty() {
                lines.push(format!("  factors: {}", candidate.risk_factors.join(", ")));
            }
        }
    }

    if !summary.notes.is_empty() {
        lines.push(String::new());
        lines.push("Notes:".to_owned());
        for note in &summary.notes {
            lines.push(format!("- {note}"));
        }
    }

    lines.join("\n")
}

fn write_rendered_output(rendered: &str, label: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    stdout
        .write_all(rendered.as_bytes())
        .with_context(|| format!("failed to write {label} output"))?;
    if !rendered.ends_with('\n') {
        writeln!(&mut stdout).with_context(|| format!("failed to terminate {label} output"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::Path;

    use aether_analysis::PreparedRefactorCandidate;
    use aether_config::load_workspace_config;
    use aether_core::Language;
    use aether_store::{
        SirMetaRecord, SirStateStore, SnapshotStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
    };
    use tempfile::tempdir;

    use super::{DeepScanOutcome, execute_refactor_prep_with_executor};
    use crate::cli::{RefactorPrepArgs, RefactorPrepOutputFormat};

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[sir_quality]
deep_concurrency = 1
deep_timeout_secs = 30

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn write_demo_source(workspace: &Path) -> String {
        let relative = "crates/demo/src/lib.rs";
        let absolute = workspace.join(relative);
        fs::create_dir_all(absolute.parent().expect("parent")).expect("mkdirs");
        fs::write(
            &absolute,
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n",
        )
        .expect("write source");
        fs::write(
            workspace.join("crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("write cargo");
        relative.to_owned()
    }

    fn parse_symbols(workspace: &Path, relative: &str) -> Vec<aether_core::Symbol> {
        let absolute = workspace.join(relative);
        let source = fs::read_to_string(&absolute).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        extractor
            .extract_from_source(Language::Rust, relative, &source)
            .expect("parse symbols")
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

    fn seed_scan_sir(store: &SqliteStore, symbol: &aether_core::Symbol) {
        let sir_json = format!(
            "{{\"confidence\":0.5,\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"{} baseline\",\"outputs\":[],\"side_effects\":[]}}",
            symbol.qualified_name
        );
        store
            .write_sir_blob(symbol.id.as_str(), &sir_json)
            .expect("write sir");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: format!("hash-{}", symbol.id),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "scan".to_owned(),
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert meta");
    }

    #[test]
    fn refactor_prep_with_mock_executor_produces_snapshot_and_brief() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        let relative = write_demo_source(temp.path());
        let store = SqliteStore::open(temp.path()).expect("open store");
        let symbols = parse_symbols(temp.path(), &relative);
        for symbol in &symbols {
            store
                .upsert_symbol(symbol_record(symbol))
                .expect("upsert symbol");
            seed_scan_sir(&store, symbol);
        }
        let config = load_workspace_config(temp.path()).expect("load config");

        let execution = execute_refactor_prep_with_executor(
            temp.path(),
            &config,
            RefactorPrepArgs {
                file: Some(relative.clone()),
                crate_name: None,
                top_n: 2,
                local: false,
                output: RefactorPrepOutputFormat::Human,
            },
            |store, candidates: &[PreparedRefactorCandidate], _local| {
                let mut succeeded = HashSet::new();
                for candidate in candidates {
                    let sir_json = format!(
                        "{{\"confidence\":0.95,\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"{} deep\",\"outputs\":[],\"side_effects\":[]}}",
                        candidate.symbol.qualified_name
                    );
                    store.write_sir_blob(candidate.symbol.id.as_str(), &sir_json)?;
                    store.upsert_sir_meta(SirMetaRecord {
                        id: candidate.symbol.id.clone(),
                        sir_hash: format!("deep-{}", candidate.symbol.id),
                        sir_version: 2,
                        provider: "mock".to_owned(),
                        model: "mock".to_owned(),
                        generation_pass: "deep".to_owned(),
                        updated_at: 1_700_000_010,
                        sir_status: "fresh".to_owned(),
                        last_error: None,
                        last_attempt_at: 1_700_000_010,
                    })?;
                    succeeded.insert(candidate.symbol.id.clone());
                }
                Ok(DeepScanOutcome {
                    requested: candidates.len(),
                    succeeded_ids: succeeded,
                    failed_symbol_ids: Vec::new(),
                })
            },
        )
        .expect("execute refactor prep");

        assert!(execution.summary.snapshot_id.starts_with("refactor-prep-"));
        assert_eq!(
            execution.summary.deep_completed,
            execution.summary.deep_requested
        );
        assert!(execution.rendered.contains("Snapshot:"));
        assert!(
            store
                .get_snapshot(execution.summary.snapshot_id.as_str())
                .expect("lookup snapshot")
                .is_some()
        );
    }
}
