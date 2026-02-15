use std::fs;

use aether_core::{SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED, SearchMode};
use aether_mcp::{
    AetherCallChainRequest, AetherDependenciesRequest, AetherExplainRequest, AetherGetSirRequest,
    AetherMcpServer, AetherSearchRequest, AetherSymbolLookupRequest, AetherSymbolTimelineRequest,
    AetherVerifyMode, AetherVerifyRequest, AetherWhyChangedReason, AetherWhyChangedRequest,
    AetherWhySelectorMode, MCP_SCHEMA_VERSION, SirLevelRequest,
};
use aether_sir::{synthetic_file_sir_id, synthetic_module_sir_id};
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
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "mock"
vector_backend = "sqlite"
"#,
    )?;

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
    let py_file = workspace.join("src/jobs.py");
    fs::write(
        &py_file,
        "def compute_total(x: int, y: int) -> int:\n    return x + y\n",
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
    assert_eq!(status.schema_version, MCP_SCHEMA_VERSION);
    assert!(status.generated_at > 0);
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
    assert_eq!(lookup.query, "alpha");
    assert_eq!(lookup.limit, 20);
    assert_eq!(lookup.mode_requested, SearchMode::Lexical);
    assert_eq!(lookup.mode_used, SearchMode::Lexical);
    assert_eq!(lookup.fallback_reason, None);
    assert!(!lookup.matches.is_empty());
    assert_eq!(lookup.result_count as usize, lookup.matches.len());
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
            mode: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(search.query, "app.ts");
    assert_eq!(search.limit, 10);
    assert_eq!(search.mode_requested, SearchMode::Lexical);
    assert_eq!(search.mode_used, SearchMode::Lexical);
    assert_eq!(search.fallback_reason, None);
    assert!(!search.matches.is_empty());
    assert_eq!(search.result_count as usize, search.matches.len());
    assert!(
        search
            .matches
            .iter()
            .any(|item| item.file_path.contains("src/app.ts"))
    );

    let python_search = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "compute_total".to_owned(),
            limit: Some(10),
            mode: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(
        python_search
            .matches
            .iter()
            .any(|item| item.file_path.contains("src/jobs.py") && item.language == "python")
    );

    let search_with_zero_limit = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "app.ts".to_owned(),
            limit: Some(0),
            mode: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(search_with_zero_limit.limit, 1);
    assert!(!search_with_zero_limit.matches.is_empty());
    assert_eq!(
        search_with_zero_limit.result_count as usize,
        search_with_zero_limit.matches.len()
    );

    let semantic_search = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "alpha summary".to_owned(),
            limit: Some(10),
            mode: Some(SearchMode::Semantic),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(semantic_search.mode_requested, SearchMode::Semantic);
    assert_eq!(semantic_search.mode_used, SearchMode::Semantic);
    assert_eq!(semantic_search.fallback_reason, None);
    assert!(!semantic_search.matches.is_empty());
    assert_eq!(
        semantic_search.result_count as usize,
        semantic_search.matches.len()
    );
    assert!(semantic_search.matches[0].semantic_score.is_some());

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
    assert!(explain.hover_markdown.contains("### alpha"));
    assert!(explain.hover_markdown.contains("Mock summary for alpha"));
    assert!(explain.hover_markdown.contains("**Confidence:**"));
    assert_eq!(explain.sir_status.as_deref(), Some("fresh"));
    assert_eq!(explain.last_error, None);
    assert!(explain.last_attempt_at.unwrap_or_default() > 0);

    let sir = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: None,
            symbol_id: Some(explain.symbol_id.clone()),
            file_path: None,
            module_path: None,
            language: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(sir.found);
    assert_eq!(sir.level, SirLevelRequest::Leaf);
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
            level: None,
            symbol_id: Some(explain.symbol_id.clone()),
            file_path: None,
            module_path: None,
            language: None,
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
    assert!(
        stale_explain
            .hover_markdown
            .contains("> AETHER WARNING: SIR is stale. Last error: provider timeout")
    );
    assert!(stale_explain.last_attempt_at.unwrap_or_default() > existing_meta.last_attempt_at);

    let sir_dir = workspace.join(".aether/sir");
    for entry in fs::read_dir(&sir_dir)? {
        let path = entry?.path();
        fs::remove_file(path)?;
    }

    let sir_without_mirror = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: None,
            symbol_id: Some(explain.symbol_id.clone()),
            file_path: None,
            module_path: None,
            language: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(sir_without_mirror.found);
    assert!(!sir_without_mirror.sir_json.is_empty());

    Ok(())
}

#[test]
fn mcp_get_sir_supports_level_requests_and_module_coverage() -> Result<()> {
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
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "mock"
vector_backend = "sqlite"
"#,
    )?;

    fs::create_dir_all(workspace.join("src/moda"))?;
    fs::write(workspace.join("src/moda/a.rs"), "fn alpha() -> i32 { 1 }\n")?;
    fs::write(workspace.join("src/moda/b.rs"), "fn beta() -> i32 { 2 }\n")?;

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

    let store = SqliteStore::open(workspace)?;
    let alpha_id = store
        .list_symbols_for_file("src/moda/a.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "alpha")
        .expect("alpha symbol should exist")
        .id;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let leaf = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: Some(SirLevelRequest::Leaf),
            symbol_id: Some(alpha_id),
            file_path: None,
            module_path: None,
            language: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(leaf.found);
    assert_eq!(leaf.level, SirLevelRequest::Leaf);
    assert!(leaf.sir.is_some());
    assert!(leaf.rollup.is_none());

    let file = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: Some(SirLevelRequest::File),
            symbol_id: None,
            file_path: Some("src/moda/a.rs".to_owned()),
            module_path: None,
            language: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(file.found);
    assert_eq!(file.level, SirLevelRequest::File);
    assert!(file.sir.is_none());
    assert!(file.rollup.is_some());

    let module_id = synthetic_module_sir_id("rust", "src/moda");
    let before = store.read_sir_blob(&module_id)?;
    assert!(before.is_none());

    let file_rollup_b = synthetic_file_sir_id("rust", "src/moda/b.rs");
    store.mark_removed(&file_rollup_b)?;

    let module = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: Some(SirLevelRequest::Module),
            symbol_id: None,
            file_path: None,
            module_path: Some("src/moda".to_owned()),
            language: Some("rust".to_owned()),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(module.found);
    assert_eq!(module.level, SirLevelRequest::Module);
    assert!(module.sir.is_none());
    assert!(module.rollup.is_some());
    assert_eq!(module.files_total, Some(2));
    assert_eq!(module.files_with_sir, Some(1));

    let after = store.read_sir_blob(&module_id)?;
    assert!(after.is_some());

    Ok(())
}

#[test]
fn mcp_dependencies_returns_callers_and_dependencies() -> Result<()> {
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
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "mock"
vector_backend = "sqlite"
"#,
    )?;

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "fn delta() -> i32 { 1 }\nfn gamma() -> i32 { delta() }\nfn beta() -> i32 { gamma() }\nfn alpha() -> i32 { beta() }\n",
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

    let store = SqliteStore::open(workspace)?;
    let beta_id = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "beta")
        .expect("beta symbol should exist")
        .id;
    let alpha_id = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "alpha")
        .expect("alpha symbol should exist")
        .id;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let beta_edges = rt
        .block_on(
            server
                .aether_dependencies(Parameters(AetherDependenciesRequest { symbol_id: beta_id })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(beta_edges.found);
    assert_eq!(beta_edges.caller_count, 1);
    assert_eq!(beta_edges.callers.len(), 1);
    assert_eq!(beta_edges.callers[0].qualified_name, "alpha");

    let alpha_edges = rt
        .block_on(
            server.aether_dependencies(Parameters(AetherDependenciesRequest {
                symbol_id: alpha_id.clone(),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(alpha_edges.found);
    assert_eq!(alpha_edges.dependency_count, 1);
    assert!(
        alpha_edges
            .dependencies
            .iter()
            .any(|edge| edge.qualified_name == "beta")
    );

    let call_chain = rt
        .block_on(server.aether_call_chain(Parameters(AetherCallChainRequest {
            symbol_id: Some(alpha_id),
            qualified_name: None,
            max_depth: Some(3),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(call_chain.found);
    assert_eq!(call_chain.depth_count, 3);
    assert_eq!(call_chain.levels.len(), 3);
    assert_eq!(call_chain.levels[0][0].qualified_name, "beta");
    assert_eq!(call_chain.levels[1][0].qualified_name, "gamma");
    assert_eq!(call_chain.levels[2][0].qualified_name, "delta");

    Ok(())
}

#[test]
fn mcp_semantic_search_falls_back_when_store_not_initialized() -> Result<()> {
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
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "mock"
vector_backend = "sqlite"
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let response = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "alpha".to_owned(),
            limit: Some(10),
            mode: Some(SearchMode::Semantic),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.mode_used, SearchMode::Lexical);
    assert_eq!(
        response.fallback_reason.as_deref(),
        Some(SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED)
    );
    assert_eq!(response.mode_requested, SearchMode::Semantic);
    assert_eq!(response.result_count, 0);
    assert!(response.matches.is_empty());

    Ok(())
}

#[test]
fn mcp_symbol_timeline_returns_expected_commit_order_and_hashes() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    let store = SqliteStore::open(workspace)?;
    store.record_sir_version_if_changed(
        "sym-alpha",
        "hash-a",
        "mock",
        "mock",
        "{\"intent\":\"v1\"}",
        1_700_100_100,
        Some("1111111111111111111111111111111111111111"),
    )?;
    store.record_sir_version_if_changed(
        "sym-alpha",
        "hash-b",
        "mock",
        "mock",
        "{\"intent\":\"v2\"}",
        1_700_100_200,
        Some("2222222222222222222222222222222222222222"),
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let response = rt
        .block_on(
            server.aether_symbol_timeline(Parameters(AetherSymbolTimelineRequest {
                symbol_id: "sym-alpha".to_owned(),
                limit: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(response.found);
    assert_eq!(response.symbol_id, "sym-alpha");
    assert_eq!(response.result_count, 2);
    assert_eq!(response.timeline.len(), 2);
    assert_eq!(response.timeline[0].version, 1);
    assert_eq!(
        response.timeline[0].commit_hash.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(response.timeline[1].version, 2);
    assert_eq!(
        response.timeline[1].commit_hash.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );

    let limited = rt
        .block_on(
            server.aether_symbol_timeline(Parameters(AetherSymbolTimelineRequest {
                symbol_id: "sym-alpha".to_owned(),
                limit: Some(1),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(limited.result_count, 1);
    assert_eq!(limited.timeline[0].version, 2);
    assert_eq!(
        limited.timeline[0].commit_hash.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );

    Ok(())
}

#[test]
fn mcp_symbol_timeline_reports_null_commit_hash_when_unavailable() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    let store = SqliteStore::open(workspace)?;
    store.record_sir_version_if_changed(
        "sym-no-git",
        "hash-a",
        "mock",
        "mock",
        "{\"intent\":\"v1\"}",
        1_700_200_100,
        None,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let response = rt
        .block_on(
            server.aether_symbol_timeline(Parameters(AetherSymbolTimelineRequest {
                symbol_id: "sym-no-git".to_owned(),
                limit: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(response.found);
    assert_eq!(response.result_count, 1);
    assert_eq!(response.timeline.len(), 1);
    assert_eq!(response.timeline[0].version, 1);
    assert_eq!(response.timeline[0].commit_hash, None);

    Ok(())
}

#[test]
fn mcp_why_changed_returns_deterministic_diff_and_commit_linkage() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    let store = SqliteStore::open(workspace)?;
    store.record_sir_version_if_changed(
        "sym-why",
        "hash-a",
        "mock",
        "mock",
        r#"{
            "intent":"v1",
            "inputs":["a"],
            "outputs":["x"],
            "side_effects":[],
            "dependencies":[],
            "error_modes":[],
            "confidence":0.5,
            "legacy_hint":"old"
        }"#,
        1_700_400_100,
        Some("1111111111111111111111111111111111111111"),
    )?;
    store.record_sir_version_if_changed(
        "sym-why",
        "hash-b",
        "mock",
        "mock",
        r#"{
            "intent":"v2",
            "inputs":["a","b"],
            "outputs":["x"],
            "side_effects":[],
            "dependencies":["serde"],
            "error_modes":[],
            "confidence":0.8,
            "new_hint":"new"
        }"#,
        1_700_400_200,
        Some("2222222222222222222222222222222222222222"),
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let request = AetherWhyChangedRequest {
        symbol_id: "sym-why".to_owned(),
        from_version: Some(1),
        to_version: Some(2),
        from_created_at: None,
        to_created_at: None,
    };
    let first = rt
        .block_on(server.aether_why_changed(Parameters(request.clone())))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let second = rt
        .block_on(server.aether_why_changed(Parameters(request)))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(first, second);
    assert!(first.found);
    assert_eq!(first.selector_mode, AetherWhySelectorMode::Version);
    assert_eq!(first.reason, None);
    assert_eq!(first.prior_summary.as_deref(), Some("v1"));
    assert_eq!(first.current_summary.as_deref(), Some("v2"));
    assert_eq!(first.fields_added, vec!["new_hint".to_owned()]);
    assert_eq!(first.fields_removed, vec!["legacy_hint".to_owned()]);
    assert_eq!(
        first.fields_modified,
        vec![
            "confidence".to_owned(),
            "dependencies".to_owned(),
            "inputs".to_owned(),
            "intent".to_owned(),
        ]
    );
    assert_eq!(
        first
            .from
            .as_ref()
            .and_then(|row| row.commit_hash.as_deref()),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(
        first.to.as_ref().and_then(|row| row.commit_hash.as_deref()),
        Some("2222222222222222222222222222222222222222")
    );

    let as_json = serde_json::to_value(&first)?;
    let object = as_json
        .as_object()
        .expect("why response should serialize as object");
    for key in [
        "symbol_id",
        "found",
        "reason",
        "selector_mode",
        "from",
        "to",
        "prior_summary",
        "current_summary",
        "fields_added",
        "fields_removed",
        "fields_modified",
    ] {
        assert!(object.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_why_changed_handles_no_history_and_single_version_fallbacks() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let no_history = rt
        .block_on(
            server.aether_why_changed(Parameters(AetherWhyChangedRequest {
                symbol_id: "sym-missing".to_owned(),
                from_version: None,
                to_version: None,
                from_created_at: None,
                to_created_at: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!no_history.found);
    assert_eq!(no_history.reason, Some(AetherWhyChangedReason::NoHistory));
    assert!(no_history.fields_added.is_empty());
    assert!(no_history.fields_removed.is_empty());
    assert!(no_history.fields_modified.is_empty());

    let store = SqliteStore::open(workspace)?;
    store.record_sir_version_if_changed(
        "sym-single",
        "hash-a",
        "mock",
        "mock",
        r#"{
            "intent":"v1",
            "inputs":["a"],
            "outputs":["x"],
            "side_effects":[],
            "dependencies":[],
            "error_modes":[],
            "confidence":0.5
        }"#,
        1_700_500_100,
        None,
    )?;

    let single = rt
        .block_on(
            server.aether_why_changed(Parameters(AetherWhyChangedRequest {
                symbol_id: "sym-single".to_owned(),
                from_version: None,
                to_version: None,
                from_created_at: None,
                to_created_at: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(single.found);
    assert_eq!(
        single.reason,
        Some(AetherWhyChangedReason::SingleVersionOnly)
    );
    assert_eq!(single.selector_mode, AetherWhySelectorMode::Auto);
    assert_eq!(single.from.as_ref().map(|row| row.version), Some(1));
    assert_eq!(single.to.as_ref().map(|row| row.version), Some(1));
    assert!(single.fields_added.is_empty());
    assert!(single.fields_removed.is_empty());
    assert!(single.fields_modified.is_empty());

    Ok(())
}

#[test]
fn mcp_why_changed_supports_timestamp_selector_mode() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    let store = SqliteStore::open(workspace)?;
    store.record_sir_version_if_changed(
        "sym-ts",
        "hash-a",
        "mock",
        "mock",
        r#"{
            "intent":"v1",
            "inputs":["a"],
            "outputs":["x"],
            "side_effects":[],
            "dependencies":[],
            "error_modes":[],
            "confidence":0.5
        }"#,
        1_700_600_100,
        None,
    )?;
    store.record_sir_version_if_changed(
        "sym-ts",
        "hash-b",
        "mock",
        "mock",
        r#"{
            "intent":"v2",
            "inputs":["a"],
            "outputs":["x","y"],
            "side_effects":[],
            "dependencies":[],
            "error_modes":[],
            "confidence":0.5
        }"#,
        1_700_600_200,
        None,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_why_changed(Parameters(AetherWhyChangedRequest {
                symbol_id: "sym-ts".to_owned(),
                from_version: None,
                to_version: None,
                from_created_at: Some(1_700_600_150),
                to_created_at: Some(1_700_600_250),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(response.found);
    assert_eq!(response.selector_mode, AetherWhySelectorMode::Timestamp);
    assert_eq!(response.from.as_ref().map(|row| row.version), Some(1));
    assert_eq!(response.to.as_ref().map(|row| row.version), Some(2));
    assert_eq!(
        response.fields_modified,
        vec!["intent".to_owned(), "outputs".to_owned()]
    );

    Ok(())
}

#[test]
fn mcp_verify_runs_allowlisted_subset_and_has_stable_response_shape() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[verify]
commands = ["cargo --version", "cargo --definitely-invalid-flag"]
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let response = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: Some(vec!["cargo --version".to_owned()]),
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(response.mode, "host");
    assert_eq!(response.mode_requested, "host");
    assert_eq!(response.mode_used, "host");
    assert_eq!(response.fallback_reason, None);
    assert_eq!(
        response.allowlisted_commands,
        vec![
            "cargo --version".to_owned(),
            "cargo --definitely-invalid-flag".to_owned()
        ]
    );
    assert_eq!(
        response.requested_commands,
        vec!["cargo --version".to_owned()]
    );
    assert!(response.passed);
    assert_eq!(response.error, None);
    assert_eq!(response.result_count, 1);
    assert_eq!(response.result_count as usize, response.results.len());
    assert_eq!(response.results[0].command, "cargo --version");
    assert_eq!(response.results[0].exit_code, Some(0));
    assert!(response.results[0].passed);
    assert!(response.results[0].stdout.contains("cargo"));

    let as_json = serde_json::to_value(&response)?;
    let object = as_json
        .as_object()
        .expect("verify response should serialize as object");
    for key in [
        "schema_version",
        "workspace",
        "mode",
        "mode_requested",
        "mode_used",
        "fallback_reason",
        "allowlisted_commands",
        "requested_commands",
        "passed",
        "error",
        "result_count",
        "results",
    ] {
        assert!(object.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_verify_reports_failure_status_output_and_allowlist_rejection() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[verify]
commands = ["cargo --version", "cargo --definitely-invalid-flag"]
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let failure = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!failure.passed);
    assert_eq!(failure.error, None);
    assert_eq!(failure.result_count as usize, failure.results.len());
    assert_eq!(failure.results.len(), 2);
    assert_eq!(failure.results[0].command, "cargo --version");
    assert_eq!(failure.results[0].exit_code, Some(0));
    assert_eq!(
        failure.results[1].command,
        "cargo --definitely-invalid-flag"
    );
    assert_ne!(failure.results[1].exit_code, Some(0));
    assert!(!failure.results[1].passed);
    assert!(
        !failure.results[1].stderr.trim().is_empty()
            || !failure.results[1].stdout.trim().is_empty()
    );

    let rejected = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: Some(vec!["cargo --not-in-allowlist".to_owned()]),
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!rejected.passed);
    assert!(rejected.results.is_empty());
    assert_eq!(rejected.result_count, 0);
    assert_eq!(
        rejected.error.as_deref(),
        Some("requested command is not allowlisted: cargo --not-in-allowlist")
    );

    Ok(())
}

#[test]
fn mcp_verify_handles_unavailable_container_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[verify]
mode = "container"
commands = ["cargo --version"]

[verify.container]
runtime = "definitely-missing-container-runtime"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = false
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let no_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!no_fallback.passed);
    assert_eq!(no_fallback.mode, "container");
    assert_eq!(no_fallback.mode_requested, "container");
    assert_eq!(no_fallback.mode_used, "container");
    assert_eq!(no_fallback.fallback_reason, None);
    assert!(no_fallback.results.is_empty());
    assert!(
        no_fallback
            .error
            .as_deref()
            .is_some_and(|message| message.contains("container runtime unavailable"))
    );

    let force_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: Some(AetherVerifyMode::Container),
            fallback_to_host_on_unavailable: Some(true),
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(force_fallback.passed);
    assert_eq!(force_fallback.mode, "host");
    assert_eq!(force_fallback.mode_requested, "container");
    assert_eq!(force_fallback.mode_used, "host");
    assert_eq!(force_fallback.result_count, 1);
    assert_eq!(force_fallback.results[0].command, "cargo --version");
    assert_eq!(force_fallback.results[0].exit_code, Some(0));
    assert!(
        force_fallback
            .fallback_reason
            .as_deref()
            .is_some_and(|message| message.contains("container runtime unavailable"))
    );

    Ok(())
}

#[test]
fn mcp_verify_handles_unavailable_microvm_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(workspace.join("vmlinux"), "")?;
    fs::write(workspace.join("rootfs.ext4"), "")?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[verify]
mode = "microvm"
commands = ["cargo --version"]

[verify.container]
runtime = "definitely-missing-container-runtime"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = false

[verify.microvm]
runtime = "definitely-missing-microvm-runtime"
kernel_image = "./vmlinux"
rootfs_image = "./rootfs.ext4"
workdir = "/workspace"
vcpu_count = 1
memory_mib = 1024
fallback_to_container_on_unavailable = false
fallback_to_host_on_unavailable = false
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let no_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!no_fallback.passed);
    assert_eq!(no_fallback.mode, "microvm");
    assert_eq!(no_fallback.mode_requested, "microvm");
    assert_eq!(no_fallback.mode_used, "microvm");
    assert_eq!(no_fallback.fallback_reason, None);
    assert!(no_fallback.results.is_empty());
    assert!(
        no_fallback
            .error
            .as_deref()
            .is_some_and(|message| message.contains("microvm runtime unavailable"))
    );

    let fallback_chain = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: Some(AetherVerifyMode::Microvm),
            fallback_to_host_on_unavailable: Some(true),
            fallback_to_container_on_unavailable: Some(true),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(fallback_chain.passed);
    assert_eq!(fallback_chain.mode, "host");
    assert_eq!(fallback_chain.mode_requested, "microvm");
    assert_eq!(fallback_chain.mode_used, "host");
    assert_eq!(fallback_chain.result_count, 1);
    assert_eq!(fallback_chain.results[0].command, "cargo --version");
    assert_eq!(fallback_chain.results[0].exit_code, Some(0));
    assert!(
        fallback_chain
            .fallback_reason
            .as_deref()
            .is_some_and(|message| message.contains("microvm runtime unavailable"))
    );

    Ok(())
}
