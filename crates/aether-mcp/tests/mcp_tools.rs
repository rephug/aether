use std::fs;

use aether_mcp::{
    AetherExplainRequest, AetherGetSirRequest, AetherMcpServer, AetherSymbolLookupRequest,
};
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

    let sir = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            symbol_id: explain.symbol_id.clone(),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(sir.found);
    let sir_annotation = sir.sir.expect("sir should be present");
    assert!(sir_annotation.intent.contains("Mock summary for"));

    Ok(())
}
