//! Search fallbacks, symbol timelines and `aether_why_changed`.

use super::*;

#[test]
fn mcp_search_hybrid_falls_back_when_embedding_api_key_is_missing() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn alpha_search_target() -> i32 { 1 }\n",
    )?;
    run_initial_index_once(&IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        sir_concurrency: 2,
        lifecycle_logs: false,
        force: false,
        full: false,
        deep: false,
        turbo_concurrency: None,
        dry_run: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        embeddings_only: false,
        pause_flag: None,
    })?;

    let env_name = unique_env_name("AETHER_TEST_MCP_MISSING_EMBED_KEY");
    unsafe {
        std::env::remove_var(&env_name);
    }

    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = true;
    config.embeddings.provider = EmbeddingProviderKind::OpenAiCompat;
    config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
    config.embeddings.model = Some("text-embedding-3-large".to_owned());
    config.embeddings.endpoint = Some("https://example.invalid/v1".to_owned());
    config.embeddings.api_key_env = Some(env_name.clone());
    save_workspace_config(workspace, &config)?;

    let state = SharedState::open_readwrite(workspace)?;
    assert!(!state.semantic_search_available);

    let expected_reason = format!(
        "Embedding API key not configured. Register MCP server with --env {env_name}=<value> to enable semantic search."
    );
    let server = AetherMcpServer::from_state(std::sync::Arc::new(state), false);
    let rt = Runtime::new()?;
    let response = rt
        .block_on(server.aether_search(Parameters(AetherSearchRequest {
            query: "alpha_search_target".to_owned(),
            limit: Some(10),
            mode: Some(SearchMode::Hybrid),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.mode_requested, SearchMode::Hybrid);
    assert_eq!(response.mode_used, SearchMode::Lexical);
    assert_eq!(
        response.fallback_reason.as_deref(),
        Some(expected_reason.as_str())
    );
    assert!(
        response
            .matches
            .iter()
            .any(|row| row.qualified_name.contains("alpha_search_target"))
    );

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
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
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
    let fallback_reason = response.fallback_reason.as_deref().unwrap_or_default();
    assert!(
        fallback_reason == SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY
            || fallback_reason.starts_with("embedding provider error:")
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
    write_test_config(workspace);

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
    write_test_config(workspace);

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
    write_test_config(workspace);

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
    write_test_config(workspace);
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
    write_test_config(workspace);

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
