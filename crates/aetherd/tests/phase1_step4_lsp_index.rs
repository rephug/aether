use std::fs;

use aether_lsp::resolve_hover_markdown_for_path;
use aether_store::SqliteStore;
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;
use tower_lsp::lsp_types::Position;

#[test]
fn lsp_index_mode_path_generates_sir_and_hover_reads_it() -> Result<(), Box<dyn std::error::Error>>
{
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

    let config = IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        sir_concurrency: 2,
        lifecycle_logs: true,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
    };

    run_initial_index_once(&config)?;

    assert!(workspace.join(".aether/meta.sqlite").exists());

    let sir_dir = workspace.join(".aether/sir");
    let sir_files = fs::read_dir(&sir_dir)?.count();
    assert!(sir_files >= 4);

    let store = SqliteStore::open(workspace)?;

    let rust_hover =
        resolve_hover_markdown_for_path(workspace, &store, &rust_file, Position::new(0, 4))?
            .expect("rust hover should resolve");

    assert!(rust_hover.contains("Mock summary for alpha"));
    assert!(rust_hover.contains("**Confidence:**"));
    assert!(rust_hover.contains("**Intent**"));

    let ts_hover =
        resolve_hover_markdown_for_path(workspace, &store, &ts_file, Position::new(1, 10))?
            .expect("ts hover should resolve");

    assert!(ts_hover.contains("Mock summary for delta"));
    assert!(ts_hover.contains("**Confidence:**"));
    assert!(ts_hover.contains("**Dependencies**"));

    Ok(())
}
