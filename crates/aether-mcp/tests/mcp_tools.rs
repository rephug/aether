use std::fs;

use aether_mcp::{
    AetherExplainRequest, AetherGetSirRequest, AetherMcpServer, AetherSearchRequest,
    AetherSymbolLookupRequest,
};
use aether_store::{SqliteStore, Store};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use tempfile::tempdir;
use tokio::runtime::Runtime;

#[test]
fn mcp_tool_handlers_work_with_local_store() -> Result<()> {
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

    run_initial_index_once(&IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        sir_concurrency: 2,
        lifecycle_logs: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
    })?;

    let server = AetherMcpServer::new(workspace, false)?;

    let rt = Runtime::new()?;

    let status = rt
        .block_on(server.aether_status())
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(status.store_present);
    assert!(status.symbol_count > 0);
    assert!(status.sir_count > 0);

    let lookup = rt
        .block_on(
            server.aether_symbol_lookup(Parameters(AetherSymbolLookupRequest {
                query: "alpha".to_owned(),
                limit: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!lookup.matches.is_empty());
    assert!(
        lookup
            .matches
            .iter()
            .any(|item| item.qualified_name.contains("alpha"))
    );

    let search = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "app.ts".to_owned(),
            limit: Some(10),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!search.matches.is_empty());
    assert!(
        search
            .matches
            .iter()
            .any(|item| item.file_path.contains("src/app.ts"))
    );

    let explain = rt
        .block_on(server.aether_explain(Parameters(AetherExplainRequest {
            file_path: "src/lib.rs".to_owned(),
            line: 1,
            column: 4,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(explain.found);
    assert!(!explain.symbol_id.is_empty());
    assert!(explain.hover_markdown.contains("Mock summary for alpha"));
    assert!(explain.hover_markdown.contains("confidence:"));
    assert_eq!(explain.sir_status.as_deref(), Some("fresh"));
    assert_eq!(explain.last_error, None);
    assert!(explain.last_attempt_at.unwrap_or_default() > 0);

    let sir = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            symbol_id: explain.symbol_id.clone(),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(sir.found);
    let sir_annotation = sir.sir.expect("sir should be present");
    assert!(sir_annotation.intent.contains("Mock summary for"));
    assert_eq!(sir.sir_status.as_deref(), Some("fresh"));
    assert_eq!(sir.last_error, None);
    assert!(sir.last_attempt_at.unwrap_or_default() > 0);

    let store = SqliteStore::open(workspace)?;
    let existing_meta = store
        .get_sir_meta(&explain.symbol_id)?
        .expect("symbol should have metadata");
    store.upsert_sir_meta(aether_store::SirMetaRecord {
        id: explain.symbol_id.clone(),
        sir_hash: existing_meta.sir_hash.clone(),
        sir_version: existing_meta.sir_version,
        provider: existing_meta.provider.clone(),
        model: existing_meta.model.clone(),
        updated_at: existing_meta.updated_at,
        sir_status: "stale".to_owned(),
        last_error: Some("provider timeout".to_owned()),
        last_attempt_at: existing_meta.last_attempt_at + 1,
    })?;

    let stale_sir = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            symbol_id: explain.symbol_id.clone(),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(stale_sir.found);
    assert_eq!(stale_sir.sir_status.as_deref(), Some("stale"));
    assert_eq!(stale_sir.last_error.as_deref(), Some("provider timeout"));
    assert!(stale_sir.last_attempt_at.unwrap_or_default() > existing_meta.last_attempt_at);

    let stale_explain = rt
        .block_on(server.aether_explain(Parameters(AetherExplainRequest {
            file_path: "src/lib.rs".to_owned(),
            line: 1,
            column: 4,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(stale_explain.sir_status.as_deref(), Some("stale"));
    assert_eq!(
        stale_explain.last_error.as_deref(),
        Some("provider timeout")
    );
    assert!(stale_explain.last_attempt_at.unwrap_or_default() > existing_meta.last_attempt_at);

    Ok(())
}
