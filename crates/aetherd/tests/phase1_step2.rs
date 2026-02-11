use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use aether_config::InferenceProviderKind;
use aether_infer::{
    InferError, InferenceProvider, MockProvider, ProviderOverrides, Qwen3LocalProvider, SirContext,
};
use aether_sir::{SirAnnotation, validate_sir};
use aether_store::{SqliteStore, Store, SymbolEmbeddingRecord};
use aetherd::observer::ObserverState;
use aetherd::sir_pipeline::SirPipeline;
use tempfile::tempdir;

#[derive(Debug, Clone, Copy)]
struct HashingMockProvider;

#[async_trait::async_trait]
impl InferenceProvider for HashingMockProvider {
    async fn generate_sir(
        &self,
        symbol_text: &str,
        context: &SirContext,
    ) -> Result<SirAnnotation, InferError> {
        let normalized = symbol_text.split_whitespace().collect::<Vec<_>>().join(" ");

        Ok(SirAnnotation {
            intent: format!(
                "Hashing mock summary for {} :: {}",
                context.qualified_name, normalized
            ),
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 1.0,
        })
    }
}

fn run_git(workspace: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {:?} failed: {}", args, stderr.trim()).into());
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn init_git_repo(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    run_git(workspace, &["init"])?;
    run_git(workspace, &["config", "user.name", "Aether Test"])?;
    run_git(
        workspace,
        &["config", "user.email", "aether-test@example.com"],
    )?;
    Ok(())
}

fn commit_all(workspace: &Path, message: &str) -> Result<String, Box<dyn std::error::Error>> {
    run_git(workspace, &["add", "."])?;
    run_git(workspace, &["commit", "-m", message])?;
    run_git(workspace, &["rev-parse", "--verify", "HEAD"])
}

#[test]
fn step2_pipeline_generates_and_persists_sir_with_mock_provider()
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

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(MockProvider);
    let pipeline =
        SirPipeline::new_with_provider(workspace.to_path_buf(), 2, provider, "mock", "mock")?;

    let mut startup_stdout = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, true, &mut startup_stdout)?;
    }

    assert!(workspace.join(".aether/meta.sqlite").exists());

    let rust_symbols = store.list_symbols_for_file("src/lib.rs")?;
    let ts_symbols = store.list_symbols_for_file("src/app.ts")?;
    assert!(rust_symbols.len() >= 2);
    assert!(ts_symbols.len() >= 2);

    let mut all_symbols = Vec::new();
    all_symbols.extend(rust_symbols);
    all_symbols.extend(ts_symbols);

    for symbol in &all_symbols {
        let blob_path = workspace
            .join(".aether/sir")
            .join(format!("{}.json", symbol.id));
        assert!(blob_path.exists());

        let blob = fs::read_to_string(blob_path)?;
        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;
    }

    let startup_output = String::from_utf8(startup_stdout)?;
    assert!(startup_output.contains("SIR_STORED symbol_id="));

    fs::write(
        &rust_file,
        "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 3 }\n",
    )?;
    fs::write(
        &ts_file,
        "function gamma(): number { return 1; }\nfunction delta(): number { return 3; }\n",
    )?;

    let rust_event = observer
        .process_path(&rust_file)?
        .expect("expected rust update event");
    let ts_event = observer
        .process_path(&ts_file)?
        .expect("expected ts update event");

    assert_eq!(rust_event.updated.len(), 1);
    assert_eq!(rust_event.updated[0].name, "beta");
    assert_eq!(ts_event.updated.len(), 1);
    assert_eq!(ts_event.updated[0].name, "delta");

    let mut update_stdout = Vec::new();
    pipeline.process_event(&store, &rust_event, true, &mut update_stdout)?;
    pipeline.process_event(&store, &ts_event, true, &mut update_stdout)?;

    for symbol in rust_event.updated.iter().chain(ts_event.updated.iter()) {
        let blob = store
            .read_sir_blob(&symbol.id)?
            .expect("updated symbol blob should exist");
        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;

        let sir_meta = store
            .get_sir_meta(&symbol.id)?
            .expect("updated symbol should have sir metadata");
        assert_eq!(sir_meta.sir_status, "fresh");
        assert_eq!(sir_meta.last_error, None);
        assert!(sir_meta.last_attempt_at > 0);
    }

    let update_output = String::from_utf8(update_stdout)?;
    assert!(update_output.contains("SIR_STORED symbol_id="));

    let sir_dir = workspace.join(".aether/sir");
    for entry in fs::read_dir(&sir_dir)? {
        let path = entry?.path();
        fs::remove_file(path)?;
    }

    for symbol in &all_symbols {
        let blob = store
            .read_sir_blob(&symbol.id)?
            .expect("db-first read should still succeed after mirror removal");
        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;
    }

    drop(store);

    let reopened_store = SqliteStore::open(workspace)?;
    for symbol in &all_symbols {
        let blob = reopened_store
            .read_sir_blob(&symbol.id)?
            .expect("reopened store should read canonical sqlite SIR");
        let sir: SirAnnotation = serde_json::from_str(&blob)?;
        validate_sir(&sir)?;
    }

    Ok(())
}

#[test]
fn step2_embeddings_refresh_when_hash_changes_and_delete_on_symbol_removal()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "mock"
"#,
    )?;

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        1,
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Mock),
            ..ProviderOverrides::default()
        },
    )?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");

    let embedding_meta = store
        .get_symbol_embedding_meta(&symbol.id)?
        .expect("embedding metadata should exist");

    store.upsert_symbol_embedding(SymbolEmbeddingRecord {
        symbol_id: symbol.id.clone(),
        sir_hash: "stale-hash".to_owned(),
        provider: "mock".to_owned(),
        model: "mock-64d".to_owned(),
        embedding: vec![1.0, 0.0, 0.0],
        updated_at: embedding_meta.updated_at.saturating_sub(1),
    })?;

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let update_event = observer
        .process_path(&rust_file)?
        .expect("expected update event");
    pipeline.process_event(&store, &update_event, false, &mut sink)?;

    let refreshed_meta = store
        .get_symbol_embedding_meta(&symbol.id)?
        .expect("embedding metadata after refresh");
    assert_ne!(refreshed_meta.sir_hash, "stale-hash");
    assert_eq!(refreshed_meta.provider, "mock");
    assert_eq!(refreshed_meta.model, "mock-64d");
    assert!(refreshed_meta.embedding_dim > 0);

    fs::write(&rust_file, "")?;
    let removal_event = observer
        .process_path(&rust_file)?
        .expect("expected removal event");
    pipeline.process_event(&store, &removal_event, false, &mut sink)?;

    let after_remove = store.get_symbol_embedding_meta(&symbol.id)?;
    assert!(after_remove.is_none());

    Ok(())
}

#[test]
fn step2_pipeline_marks_stale_on_failure_and_clears_on_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let mock_provider: Arc<dyn InferenceProvider> = Arc::new(MockProvider);
    let success_pipeline =
        SirPipeline::new_with_provider(workspace.to_path_buf(), 1, mock_provider, "mock", "mock")?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        success_pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");
    let baseline_blob = store
        .read_sir_blob(&symbol.id)?
        .expect("baseline SIR blob should exist");

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let stale_event = observer
        .process_path(&rust_file)?
        .expect("expected stale update event");

    let failing_provider: Arc<dyn InferenceProvider> = Arc::new(Qwen3LocalProvider::new(
        Some("http://127.0.0.1:9".to_owned()),
        Some("qwen3".to_owned()),
    ));
    let failing_pipeline = SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        failing_provider,
        "qwen3_local",
        "qwen3",
    )?;
    failing_pipeline.process_event(&store, &stale_event, false, &mut sink)?;

    let stale_blob = store
        .read_sir_blob(&symbol.id)?
        .expect("stale flow should keep last good SIR");
    assert_eq!(stale_blob, baseline_blob);

    let stale_meta = store
        .get_sir_meta(&symbol.id)?
        .expect("stale flow should keep metadata");
    assert_eq!(stale_meta.sir_status, "stale");
    assert!(stale_meta.last_error.is_some());
    assert!(stale_meta.last_attempt_at > 0);

    fs::write(&rust_file, "fn alpha() -> i32 { 3 }\n")?;
    let recovery_event = observer
        .process_path(&rust_file)?
        .expect("expected recovery update event");
    success_pipeline.process_event(&store, &recovery_event, false, &mut sink)?;

    let recovered_meta = store
        .get_sir_meta(&symbol.id)?
        .expect("recovery flow should keep metadata");
    assert_eq!(recovered_meta.sir_status, "fresh");
    assert_eq!(recovered_meta.last_error, None);
    assert!(recovered_meta.last_attempt_at > 0);

    Ok(())
}

#[test]
fn step2_pipeline_creates_new_version_on_hash_change_without_duplicate_on_reindex()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(HashingMockProvider);
    let pipeline = SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        provider,
        "hashing_mock",
        "hashing_mock",
    )?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");
    let initial_history = store.list_sir_history(&symbol.id)?;
    assert_eq!(initial_history.len(), 1);
    assert_eq!(initial_history[0].version, 1);

    let initial_meta = store
        .get_sir_meta(&symbol.id)?
        .expect("initial metadata should exist");
    assert_eq!(initial_meta.sir_version, 1);

    let mut reindex_observer = ObserverState::new(workspace.to_path_buf())?;
    reindex_observer.seed_from_disk()?;
    for event in reindex_observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    let history_after_reindex = store.list_sir_history(&symbol.id)?;
    assert_eq!(history_after_reindex.len(), 1);
    let meta_after_reindex = store
        .get_sir_meta(&symbol.id)?
        .expect("metadata after reindex should exist");
    assert_eq!(meta_after_reindex.sir_version, 1);
    assert_eq!(meta_after_reindex.updated_at, initial_meta.updated_at);
    assert!(meta_after_reindex.last_attempt_at >= initial_meta.last_attempt_at);

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let update_event = reindex_observer
        .process_path(&rust_file)?
        .expect("expected update event");
    pipeline.process_event(&store, &update_event, false, &mut sink)?;

    let history_after_update = store.list_sir_history(&symbol.id)?;
    assert_eq!(history_after_update.len(), 2);
    assert_eq!(history_after_update[0].version, 1);
    assert_eq!(history_after_update[1].version, 2);
    assert_ne!(
        history_after_update[0].sir_hash,
        history_after_update[1].sir_hash
    );

    let meta_after_update = store
        .get_sir_meta(&symbol.id)?
        .expect("metadata after update should exist");
    assert_eq!(meta_after_update.sir_version, 2);
    assert_eq!(
        meta_after_update.updated_at,
        history_after_update[1].created_at
    );
    assert!(meta_after_update.last_attempt_at >= meta_after_update.updated_at);

    Ok(())
}

#[test]
fn step2_sir_history_retrieval_persists_after_restart() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(HashingMockProvider);
    let pipeline = SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        provider,
        "hashing_mock",
        "hashing_mock",
    )?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let update_event = observer
        .process_path(&rust_file)?
        .expect("expected update event");
    pipeline.process_event(&store, &update_event, false, &mut sink)?;

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");

    let history_before_restart = store.list_sir_history(&symbol.id)?;
    assert_eq!(history_before_restart.len(), 2);
    assert_eq!(history_before_restart[0].version, 1);
    assert_eq!(history_before_restart[1].version, 2);

    drop(store);

    let reopened = SqliteStore::open(workspace)?;
    let history_after_restart = reopened.list_sir_history(&symbol.id)?;
    assert_eq!(history_after_restart, history_before_restart);

    Ok(())
}

#[test]
fn step2_pipeline_links_sir_versions_to_git_commits() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    init_git_repo(workspace)?;
    let commit_v1 = commit_all(workspace, "initial alpha")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(HashingMockProvider);
    let pipeline = SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        provider,
        "hashing_mock",
        "hashing_mock",
    )?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");

    let initial_history = store.list_sir_history(&symbol.id)?;
    assert_eq!(initial_history.len(), 1);
    assert_eq!(initial_history[0].version, 1);
    assert_eq!(
        initial_history[0].commit_hash.as_deref(),
        Some(commit_v1.as_str())
    );

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let commit_v2 = commit_all(workspace, "update alpha")?;

    let update_event = observer
        .process_path(&rust_file)?
        .expect("expected update event");
    pipeline.process_event(&store, &update_event, false, &mut sink)?;

    let history = store.list_sir_history(&symbol.id)?;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version, 1);
    assert_eq!(history[1].version, 2);
    assert_eq!(history[0].commit_hash.as_deref(), Some(commit_v1.as_str()));
    assert_eq!(history[1].commit_hash.as_deref(), Some(commit_v2.as_str()));

    Ok(())
}

#[test]
fn step2_pipeline_records_null_commit_hash_when_git_is_unavailable()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join("src"))?;
    let rust_file = workspace.join("src/lib.rs");
    fs::write(&rust_file, "fn alpha() -> i32 { 1 }\n")?;

    let mut observer = ObserverState::new(workspace.to_path_buf())?;
    observer.seed_from_disk()?;

    let store = SqliteStore::open(workspace)?;
    let provider: Arc<dyn InferenceProvider> = Arc::new(HashingMockProvider);
    let pipeline = SirPipeline::new_with_provider(
        workspace.to_path_buf(),
        1,
        provider,
        "hashing_mock",
        "hashing_mock",
    )?;

    let mut sink = Vec::new();
    for event in observer.initial_symbol_events() {
        pipeline.process_event(&store, &event, false, &mut sink)?;
    }

    fs::write(&rust_file, "fn alpha() -> i32 { 2 }\n")?;
    let update_event = observer
        .process_path(&rust_file)?
        .expect("expected update event");
    pipeline.process_event(&store, &update_event, false, &mut sink)?;

    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|record| record.qualified_name.ends_with("alpha"))
        .expect("alpha symbol should exist");
    let history = store.list_sir_history(&symbol.id)?;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].commit_hash, None);
    assert_eq!(history[1].commit_hash, None);

    Ok(())
}
