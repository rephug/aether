use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use aether_infer::{InferenceProvider, Qwen3LocalProvider};
use aether_sir::SirAnnotation;
use aether_store::{SirMetaRecord, SirStateStore, SqliteStore};
use anyhow::{Context, Result, anyhow};

use crate::batch::hash::compute_prompt_hash;
use crate::batch::write_fingerprint_row;
use crate::cli::SirInjectArgs;
use crate::continuous::cosine_distance_from_embeddings;
use crate::sir_agent_support::{
    load_fresh_symbol_source, parse_text_list, resolve_symbol, symbol_from_record,
};
use crate::sir_pipeline::{SirPipeline, UpsertSirIntentPayload};

const FORCE_CONFIDENCE_THRESHOLD: f32 = 0.5;

#[derive(Debug)]
struct InjectExecution {
    rendered: String,
}

struct DiffPreview<'a> {
    old_intent: &'a str,
    new_intent: &'a str,
    old_side_effects: &'a [String],
    new_side_effects: &'a [String],
    old_error_modes: &'a [String],
    new_error_modes: &'a [String],
    old_confidence: f32,
    new_confidence: f32,
}

pub fn run_sir_inject_command(workspace: &Path, args: SirInjectArgs) -> Result<()> {
    let execution = execute_sir_inject_command(workspace, args)?;
    let mut out = std::io::stdout();
    out.write_all(execution.rendered.as_bytes())
        .context("failed to write sir-inject output")?;
    if !execution.rendered.ends_with('\n') {
        writeln!(&mut out).context("failed to write trailing newline")?;
    }
    Ok(())
}

fn execute_sir_inject_command(workspace: &Path, args: SirInjectArgs) -> Result<InjectExecution> {
    let store = SqliteStore::open(workspace).context("failed to open local store")?;
    let record = resolve_symbol(&store, args.selector.as_str())?;
    let fresh = load_fresh_symbol_source(workspace, &record)?;
    let existing_blob = store
        .read_sir_blob(record.id.as_str())
        .with_context(|| format!("failed to read existing SIR for {}", record.id))?;
    let existing_sir = existing_blob
        .as_deref()
        .map(serde_json::from_str::<SirAnnotation>)
        .transpose()
        .with_context(|| format!("failed to parse existing SIR for {}", record.id))?;
    let current_meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to read SIR metadata for {}", record.id))?;

    let would_block = existing_sir
        .as_ref()
        .is_some_and(|sir| sir.confidence > FORCE_CONFIDENCE_THRESHOLD)
        && !args.force;

    let mut updated = existing_sir.clone().unwrap_or_else(empty_sir);
    let old_intent = updated.intent.clone();
    let old_side_effects = updated.side_effects.clone();
    let old_error_modes = updated.error_modes.clone();
    let old_confidence = updated.confidence;

    updated.intent = args.intent.trim().to_owned();
    if let Some(behavior) = args.behavior.as_deref() {
        updated.side_effects = parse_text_list(behavior);
    }
    if let Some(edge_cases) = args.edge_cases.as_deref() {
        updated.error_modes = parse_text_list(edge_cases);
    }
    updated.confidence = FORCE_CONFIDENCE_THRESHOLD;

    if updated.intent.trim().is_empty() {
        return Err(anyhow!("intent must not be empty"));
    }

    let diff = render_diff(DiffPreview {
        old_intent: old_intent.as_str(),
        new_intent: updated.intent.as_str(),
        old_side_effects: &old_side_effects,
        new_side_effects: &updated.side_effects,
        old_error_modes: &old_error_modes,
        new_error_modes: &updated.error_modes,
        old_confidence,
        new_confidence: updated.confidence,
    });

    if args.dry_run {
        let mut rendered = String::new();
        rendered.push_str(&format!("Dry run for {}\n\n", record.qualified_name));
        rendered.push_str(diff.as_str());
        if would_block {
            rendered.push_str(&format!(
                "\nWrite would be blocked because existing confidence ({:.2}) exceeds {:.2}. Re-run with --force.\n",
                existing_sir.as_ref().map(|sir| sir.confidence).unwrap_or_default(),
                FORCE_CONFIDENCE_THRESHOLD
            ));
        }
        return Ok(InjectExecution { rendered });
    }

    if would_block {
        return Err(anyhow!(
            "existing SIR confidence ({:.2}) exceeds {:.2}; re-run with --force to overwrite",
            existing_sir
                .as_ref()
                .map(|sir| sir.confidence)
                .unwrap_or_default(),
            FORCE_CONFIDENCE_THRESHOLD
        ));
    }

    let previous_prompt_hash = current_meta
        .as_ref()
        .and_then(|meta| meta.prompt_hash.as_deref())
        .map(str::to_owned);

    let persist_pipeline = build_persist_pipeline(workspace)?;
    let mut embedding_warning = None;
    let mut previous_embedding = None;
    let mut embedding_pipeline = if args.no_embed {
        None
    } else {
        match SirPipeline::new_embeddings_only(workspace.to_path_buf()) {
            Ok(pipeline) => Some(pipeline),
            Err(err) => {
                embedding_warning = Some(format!(
                    "warning: embeddings unavailable; persisted SIR without refresh: {err}"
                ));
                None
            }
        }
    };

    if let Some(pipeline) = embedding_pipeline.as_ref() {
        previous_embedding = pipeline
            .load_symbol_embedding(record.id.as_str())
            .with_context(|| format!("failed to load existing embedding for {}", record.id))?;
    }

    let payload = UpsertSirIntentPayload {
        symbol: symbol_from_record(&record)?,
        sir: updated.clone(),
        provider_name: "manual".to_owned(),
        model_name: "manual".to_owned(),
        generation_pass: "injected".to_owned(),
        commit_hash: None,
    };
    let (canonical_json, sir_hash) = persist_pipeline
        .persist_sir_payload_into_sqlite(&store, &payload)
        .with_context(|| format!("failed to persist injected SIR for {}", record.id))?;

    let persisted_meta = store
        .get_sir_meta(record.id.as_str())
        .with_context(|| format!("failed to reload SIR metadata for {}", record.id))?
        .ok_or_else(|| anyhow!("missing persisted SIR metadata for {}", record.id))?;
    let prompt_hash = compute_prompt_hash(fresh.symbol_source.as_str(), &[], "manual:inject:0");
    store
        .upsert_sir_meta(SirMetaRecord {
            prompt_hash: Some(prompt_hash.clone()),
            staleness_score: None,
            ..persisted_meta
        })
        .with_context(|| format!("failed to persist prompt hash for {}", record.id))?;

    let delta_sem = if let Some(pipeline) = embedding_pipeline.as_mut() {
        pipeline
            .refresh_embedding_if_needed(
                record.id.as_str(),
                sir_hash.as_str(),
                canonical_json.as_str(),
                false,
                &mut std::io::sink(),
                None,
            )
            .with_context(|| format!("failed to refresh embedding for {}", record.id))?;
        let current_embedding = pipeline
            .load_symbol_embedding(record.id.as_str())
            .with_context(|| format!("failed to load refreshed embedding for {}", record.id))?;
        cosine_distance_from_embeddings(previous_embedding.as_ref(), current_embedding.as_ref())
    } else {
        None
    };

    write_fingerprint_row(
        &store,
        record.id.as_str(),
        prompt_hash.as_str(),
        previous_prompt_hash.as_deref(),
        "inject",
        "manual",
        "injected",
        delta_sem,
    )
    .with_context(|| format!("failed to write fingerprint history for {}", record.id))?;

    let mut rendered = format!(
        "Updated SIR for {}. Intent: {}\n\n{}",
        record.qualified_name,
        truncate_preview(updated.intent.as_str(), 80),
        diff
    );
    if let Some(warning) = embedding_warning {
        rendered.push('\n');
        rendered.push_str(warning.as_str());
        rendered.push('\n');
    }

    Ok(InjectExecution { rendered })
}

fn build_persist_pipeline(workspace: &Path) -> Result<SirPipeline> {
    let placeholder: Arc<dyn InferenceProvider> = Arc::new(Qwen3LocalProvider::new(None, None));
    SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        placeholder,
        "manual-placeholder",
        "manual-placeholder",
    )
    .context("failed to initialize inject persistence pipeline")
}

fn empty_sir() -> SirAnnotation {
    SirAnnotation {
        intent: String::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.0,
        method_dependencies: None,
    }
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    let mut preview = String::new();
    for ch in value.chars().take(max_chars) {
        preview.push(ch);
    }
    if value.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn render_diff(diff: DiffPreview<'_>) -> String {
    format!(
        "intent:\n- old: {}\n- new: {}\n\nbehavior:\n- old: {}\n- new: {}\n\nedge_cases:\n- old: {}\n- new: {}\n\nconfidence:\n- old: {:.2}\n- new: {:.2}\n",
        render_list_preview(diff.old_intent, false),
        render_list_preview(diff.new_intent, false),
        render_list_preview(&diff.old_side_effects.join("; "), true),
        render_list_preview(&diff.new_side_effects.join("; "), true),
        render_list_preview(&diff.old_error_modes.join("; "), true),
        render_list_preview(&diff.new_error_modes.join("; "), true),
        diff.old_confidence,
        diff.new_confidence
    )
}

fn render_list_preview(value: &str, allow_empty_label: bool) -> String {
    let value = value.trim();
    if value.is_empty() {
        if allow_empty_label {
            "(empty)".to_owned()
        } else {
            "(none)".to_owned()
        }
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::Language;
    use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::execute_sir_inject_command;
    use crate::cli::SirInjectArgs;

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

    fn seed_workspace(confidence: f32) -> (tempfile::TempDir, SymbolRecord) {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        fs::create_dir_all(temp.path().join("src")).expect("create src");
        fs::write(
            temp.path().join("src/lib.rs"),
            "pub fn alpha() -> i32 { 1 }\n",
        )
        .expect("write source");

        let source = fs::read_to_string(temp.path().join("src/lib.rs")).expect("read source");
        let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
        let symbol = extractor
            .extract_from_source(Language::Rust, "src/lib.rs", &source)
            .expect("parse")
            .into_iter()
            .next()
            .expect("symbol");
        let record = SymbolRecord {
            id: symbol.id.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.as_str().to_owned(),
            kind: symbol.kind.as_str().to_owned(),
            qualified_name: symbol.qualified_name.clone(),
            signature_fingerprint: symbol.signature_fingerprint.clone(),
            last_seen_at: 1_700_000_000,
        };
        let store = SqliteStore::open(temp.path()).expect("open store");
        store.upsert_symbol(record.clone()).expect("upsert symbol");
        store
            .write_sir_blob(
                record.id.as_str(),
                &format!(
                    "{{\"confidence\":{confidence},\"dependencies\":[],\"error_modes\":[],\"inputs\":[],\"intent\":\"old intent\",\"outputs\":[],\"side_effects\":[]}}"
                ),
            )
            .expect("write sir");
        store
            .upsert_sir_meta(aether_store::SirMetaRecord {
                id: record.id.clone(),
                sir_hash: "hash-old".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                generation_pass: "scan".to_owned(),
                prompt_hash: Some("srcold|nbrold|cfgold".to_owned()),
                staleness_score: Some(0.4),
                updated_at: 1_700_000_001,
                sir_status: "fresh".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_001,
            })
            .expect("upsert meta");
        (temp, record)
    }

    #[test]
    fn inject_dry_run_reports_changes_without_persisting() {
        let (temp, record) = seed_workspace(0.4);
        let execution = execute_sir_inject_command(
            temp.path(),
            SirInjectArgs {
                selector: record.id.clone(),
                intent: "new intent".to_owned(),
                behavior: Some("writes cache".to_owned()),
                edge_cases: Some("io".to_owned()),
                force: false,
                dry_run: true,
                no_embed: true,
            },
        )
        .expect("dry run");

        assert!(execution.rendered.contains("Dry run"));
        assert!(execution.rendered.contains("new intent"));

        let store = SqliteStore::open(temp.path()).expect("open store");
        let blob = store
            .read_sir_blob(record.id.as_str())
            .expect("read blob")
            .expect("blob");
        assert!(blob.contains("old intent"));
    }

    #[test]
    fn inject_requires_force_for_high_confidence_sir() {
        let (temp, record) = seed_workspace(0.9);
        let err = execute_sir_inject_command(
            temp.path(),
            SirInjectArgs {
                selector: record.id.clone(),
                intent: "new intent".to_owned(),
                behavior: None,
                edge_cases: None,
                force: false,
                dry_run: false,
                no_embed: true,
            },
        )
        .expect_err("should require force");

        assert!(err.to_string().contains("re-run with --force"));
    }

    #[test]
    fn inject_persists_new_intent_and_prompt_hash() {
        let (temp, record) = seed_workspace(0.4);
        execute_sir_inject_command(
            temp.path(),
            SirInjectArgs {
                selector: record.id.clone(),
                intent: "updated intent".to_owned(),
                behavior: Some("writes cache".to_owned()),
                edge_cases: Some("io".to_owned()),
                force: false,
                dry_run: false,
                no_embed: true,
            },
        )
        .expect("inject");

        let store = SqliteStore::open(temp.path()).expect("open store");
        let blob = store
            .read_sir_blob(record.id.as_str())
            .expect("read blob")
            .expect("blob");
        let meta = store
            .get_sir_meta(record.id.as_str())
            .expect("get meta")
            .expect("meta");
        assert!(blob.contains("updated intent"));
        assert_eq!(meta.generation_pass, "injected");
        assert_eq!(
            meta.prompt_hash
                .as_deref()
                .map(|value| value.split('|').count()),
            Some(3)
        );
        let history = store
            .list_sir_fingerprint_history(record.id.as_str())
            .expect("list history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].trigger, "inject");
    }
}
