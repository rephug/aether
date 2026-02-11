use std::fs;
use std::sync::Arc;

use aether_infer::{InferenceProvider, MockProvider, Qwen3LocalProvider};
use aether_sir::{SirAnnotation, validate_sir};
use aether_store::{SqliteStore, Store};
use aetherd::observer::ObserverState;
use aetherd::sir_pipeline::SirPipeline;
use tempfile::tempdir;

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
