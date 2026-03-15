use std::fs;
use std::path::Path;
use std::sync::Arc;

use aether_infer::{InferError, InferenceProvider, SirContext};
use aether_lsp::resolve_hover_markdown_for_path;
use aether_sir::SirAnnotation;
use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore};
use aetherd::observer::ObserverState;
use aetherd::sir_pipeline::SirPipeline;
use async_trait::async_trait;
use tempfile::tempdir;
use tower_lsp::lsp_types::Position;

struct TestProvider;

#[async_trait]
impl InferenceProvider for TestProvider {
    async fn generate_sir(
        &self,
        _symbol_text: &str,
        context: &SirContext,
    ) -> std::result::Result<SirAnnotation, InferError> {
        Ok(SirAnnotation {
            intent: format!("Test SIR for {}", context.qualified_name),
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.9,
            method_dependencies: None,
        })
    }
}

#[test]
fn hover_resolution_returns_test_sir_for_rust_and_ts_symbols()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;

    let rust_file = workspace.join("src/lib.rs");
    fs::write(
        &rust_file,
        "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
    )?;

    let ts_file = workspace.join("src/app.ts");
    fs::write(
        &ts_file,
        "function gamma(): number { return 1; }\nfunction delta(): number { return 2; }\n",
    )?;

    index_workspace_with_provider(workspace)?;

    let store = SqliteStore::open(workspace)?;

    let rust_hover =
        resolve_hover_markdown_for_path(workspace, &store, &rust_file, Position::new(0, 4))?
            .expect("rust hover should resolve");

    assert!(rust_hover.contains("Test SIR for"));
    assert!(rust_hover.contains("**Confidence:**"));
    assert!(rust_hover.contains("**Intent**"));

    let ts_hover =
        resolve_hover_markdown_for_path(workspace, &store, &ts_file, Position::new(1, 10))?
            .expect("ts hover should resolve");

    assert!(ts_hover.contains("Test SIR for"));
    assert!(ts_hover.contains("**Confidence:**"));
    assert!(ts_hover.contains("**Dependencies**"));

    Ok(())
}

#[test]
fn hover_resolution_shows_stale_warning_and_clears_after_fresh_meta()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    index_workspace_with_provider(workspace)?;

    let store = SqliteStore::open(workspace)?;
    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");
    let existing_meta = store
        .get_sir_meta(&symbol.id)?
        .expect("symbol should have existing sir metadata");

    store.upsert_sir_meta(aether_store::SirMetaRecord {
        id: symbol.id.clone(),
        sir_hash: existing_meta.sir_hash.clone(),
        sir_version: existing_meta.sir_version,
        provider: existing_meta.provider.clone(),
        model: existing_meta.model.clone(),
        generation_pass: "scan".to_owned(),
        prompt_hash: None,
        staleness_score: None,
        updated_at: existing_meta.updated_at,
        sir_status: "stale".to_owned(),
        last_error: Some("provider unavailable".to_owned()),
        last_attempt_at: existing_meta.last_attempt_at + 1,
    })?;

    let stale_hover =
        resolve_hover_markdown_for_path(workspace, &store, &rust_file, Position::new(0, 4))?
            .expect("stale hover should resolve");
    assert!(stale_hover.contains("AETHER WARNING: SIR is stale."));
    assert!(stale_hover.contains("provider unavailable"));

    store.upsert_sir_meta(aether_store::SirMetaRecord {
        id: symbol.id.clone(),
        sir_hash: existing_meta.sir_hash,
        sir_version: existing_meta.sir_version,
        provider: existing_meta.provider,
        model: existing_meta.model,
        generation_pass: "scan".to_owned(),
        prompt_hash: None,
        staleness_score: None,
        updated_at: existing_meta.updated_at,
        sir_status: "fresh".to_owned(),
        last_error: None,
        last_attempt_at: existing_meta.last_attempt_at + 2,
    })?;

    let fresh_hover =
        resolve_hover_markdown_for_path(workspace, &store, &rust_file, Position::new(0, 4))?
            .expect("fresh hover should resolve");
    assert!(!fresh_hover.contains("AETHER WARNING: SIR is stale."));

    Ok(())
}

fn index_workspace_with_provider(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_pipeline_config(workspace)?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(TestProvider);
    let pipeline =
        SirPipeline::new_with_provider(workspace.to_path_buf(), 2, provider, "test", "test")?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, false, &mut sink)?;
    }

    Ok(())
}

fn write_pipeline_config(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;
    Ok(())
}
