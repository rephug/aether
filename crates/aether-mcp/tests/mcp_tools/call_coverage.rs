//! Call-edge coverage: how `aether_dependencies`, `aether_call_chain` and
//! `aether_status` say what the graph couldn't resolve or attribute.

use super::*;
use aether_mcp::AetherDependenciesResponse;

/// An empty list must be distinguishable from calls the graph couldn't resolve or
/// attribute: each response says how the recorded calls resolved.
#[test]
fn dependencies_call_chain_and_status_report_call_coverage() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"

[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
vector_backend = "sqlite"
"#,
    )?;
    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "fn leaf() -> i32 { 1 }\nfn helper() -> i32 { 2 }\nfn uses() -> i32 { helper() + leaf() }\nfn external() -> String { String::new() }\nfn calls_dup() -> i32 { dup() }\nfn calls_other() -> i32 { 0 }\nfn hidden_leaf() -> i32 { 7 }\nfn pointer_call() -> i32 { let callback = hidden_leaf; callback() }\nstruct PointerTarget;\nimpl PointerTarget { fn hidden(&self) -> i32 { 1 } }\nfn method_pointer(value: &PointerTarget) -> i32 { let callback = PointerTarget::hidden; callback(value) }\n",
    )?;
    fs::write(workspace.join("src/a.rs"), "fn dup() -> i32 { 1 }\n")?;
    fs::write(workspace.join("src/b.rs"), "fn dup() -> i32 { 2 }\n")?;
    // The explicit nominal parameter type establishes the inherent method target.
    fs::write(
        workspace.join("src/shape.rs"),
        "struct Square;\nimpl Square {\n    fn area(&self) -> i32 { 4 }\n}\nfn measure(s: Square) -> i32 { s.area() }\n",
    )?;
    run_initial_index_once(&IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        sir_concurrency: 1,
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

    let store = SqliteStore::open(workspace)?;
    let id_of = |file: &str, name: &str| -> Result<String> {
        Ok(store
            .list_symbols_for_file(file)?
            .into_iter()
            .find(|symbol| symbol.qualified_name == name)
            .unwrap_or_else(|| panic!("{name} in {file}"))
            .id)
    };
    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let dependencies = |symbol_id: String| -> Result<AetherDependenciesResponse> {
        Ok(rt
            .block_on(
                server.aether_dependencies(Parameters(AetherDependenciesRequest { symbol_id })),
            )
            .map_err(|err| anyhow::anyhow!(err.to_string()))?
            .0)
    };

    // No outgoing calls are recorded; this does not certify complete program coverage.
    let leaf = dependencies(id_of("src/lib.rs", "leaf")?)?;
    assert!(leaf.dependencies.is_empty());
    assert_eq!(leaf.dependency_coverage.call_edges, 0);
    assert!(
        leaf.coverage_note
            .contains("no outgoing calls are recorded"),
        "{}",
        leaf.coverage_note
    );
    // `uses` calls it by its exact, unique name: no guess involved.
    assert_eq!(
        (
            leaf.caller_coverage.attributed,
            leaf.caller_coverage.heuristic
        ),
        (1, 0)
    );
    assert!(
        !leaf.coverage_note.contains("by heuristic"),
        "{}",
        leaf.coverage_note
    );

    // Calls only something outside the workspace: the empty list is not "calls nothing".
    let external = dependencies(id_of("src/lib.rs", "external")?)?;
    assert!(external.dependencies.is_empty());
    assert_eq!(
        (
            external.dependency_coverage.call_edges,
            external.dependency_coverage.unresolved
        ),
        (1, 1)
    );
    // Unresolved is not the same as external.
    for wording in [
        "1 lack sufficient target evidence",
        "may be internal",
        "Calls written inside macro arguments",
    ] {
        assert!(
            external.coverage_note.contains(wording),
            "{wording}: {}",
            external.coverage_note
        );
    }
    assert!(
        !external.coverage_note.contains("name no workspace symbol"),
        "{}",
        external.coverage_note
    );

    // A nominal typed receiver proves its unique inherent method without a heuristic.
    let area = dependencies(id_of("src/shape.rs", "Square::area")?)?;
    assert_eq!(
        area.callers
            .iter()
            .map(|caller| caller.qualified_name.as_str())
            .collect::<Vec<_>>(),
        ["measure"]
    );
    assert_eq!(
        (
            area.caller_coverage.attributed,
            area.caller_coverage.heuristic
        ),
        (1, 0)
    );
    assert!(
        !area.coverage_note.contains("by heuristic"),
        "{}",
        area.coverage_note
    );

    let uses = dependencies(id_of("src/lib.rs", "uses")?)?;
    assert_eq!(uses.dependency_count, 2);
    assert_eq!(
        (
            uses.dependency_coverage.call_edges,
            uses.dependency_coverage.resolved
        ),
        (2, 2)
    );

    // A bare name in unrelated files provides no import or module proof.
    let calls_dup = dependencies(id_of("src/lib.rs", "calls_dup")?)?;
    assert_eq!(calls_dup.dependency_count, 0);
    assert_eq!(calls_dup.dependency_coverage.unresolved, 1);
    assert_eq!(calls_dup.dependency_coverage.ambiguous, 0);

    // A proved different identity is separate from a missing-proof reference.
    store.upsert_edges(&[SymbolEdge {
        source_id: id_of("src/lib.rs", "calls_other")?,
        target_qualified_name: "dup".to_owned(),
        target_symbol_id: Some(id_of("src/b.rs", "dup")?),
        target_file_path: Some("src/b.rs".to_owned()),
        edge_kind: EdgeKind::Calls,
        file_path: "src/lib.rs".to_owned(),
    }])?;
    // Neither unrelated declaration receives the unproved caller.
    let dup = dependencies(id_of("src/a.rs", "dup")?)?;
    assert!(dup.callers.is_empty());
    assert_eq!(
        (
            dup.caller_coverage.name_matches,
            dup.caller_coverage.unresolved,
            dup.caller_coverage.resolved_elsewhere
        ),
        (2, 1, 1)
    );
    assert!(
        dup.coverage_note.contains("lack target proof"),
        "{}",
        dup.coverage_note
    );

    // A genuine function-pointer caller need not even match the selected name.
    let hidden = dependencies(id_of("src/lib.rs", "hidden_leaf")?)?;
    assert!(hidden.callers.is_empty());
    assert_eq!(hidden.caller_coverage.name_matches, 0);
    assert_eq!(
        serde_json::to_value(hidden.caller_coverage)?["incoming_completeness"],
        "unknown"
    );
    assert!(
        hidden
            .coverage_note
            .contains("incoming completeness is unknown")
    );
    let pointer = dependencies(id_of("src/lib.rs", "pointer_call")?)?;
    assert_eq!(pointer.dependency_coverage.unresolved, 1);
    let matrix = server.aether_usage_matrix_logic(AetherUsageMatrixRequest {
        symbol: "PointerTarget".to_owned(),
        symbol_id: None,
        file: Some("src/lib.rs".to_owned()),
        kind: Some("struct".to_owned()),
    })?;
    assert_eq!(matrix.methods_without_attributed_callers, ["hidden"]);
    assert_eq!(matrix.caller_coverage.name_matches, 0);
    let encoded = serde_json::to_value(&matrix)?;
    assert_eq!(encoded["incoming_completeness"], "unknown");
    assert!(encoded.get("uncalled_methods").is_none());

    let call_chain = |symbol_id: String| -> Result<_> {
        Ok(rt
            .block_on(server.aether_call_chain(Parameters(AetherCallChainRequest {
                symbol_id: Some(symbol_id),
                qualified_name: None,
                max_depth: Some(3),
            })))
            .map_err(|err| anyhow::anyhow!(err.to_string()))?
            .0)
    };
    let leaf_chain = call_chain(id_of("src/lib.rs", "leaf")?)?;
    assert!(leaf_chain.levels.is_empty());
    assert_eq!(leaf_chain.coverage.call_edges, 0);
    let external_chain = call_chain(id_of("src/lib.rs", "external")?)?;
    assert!(external_chain.levels.is_empty());
    assert_eq!(external_chain.coverage.unresolved, 1);
    assert!(
        external_chain
            .coverage_note
            .contains("No unique workspace target was resolved for the recorded calls."),
        "{}",
        external_chain.coverage_note
    );

    let status = rt
        .block_on(server.aether_status())
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let rust = status
        .call_graph
        .languages
        .iter()
        .find(|language| language.language == "rust")
        .expect("rust coverage");
    assert_eq!(
        (
            rust.call_edges,
            rust.resolved,
            rust.ambiguous,
            rust.unresolved
        ),
        (8, 4, 0, 4)
    );
    assert!(status.call_graph.rule.contains("exact qualified name"));
    assert!(status.call_graph.rule.contains("may be internal"));
    assert!(status.call_graph.rule.contains("macro arguments"));

    Ok(())
}

#[test]
fn cached_native_health_requires_current_projection_but_coupling_checkout_remains_available()
-> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Surreal;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config)?;
    let runtime = Runtime::new()?;
    let state = runtime.block_on(SharedState::open_readwrite_async(temp.path()))?;
    let cached = runtime.block_on(state.surreal_graph())?;
    assert!(state.store.has_graph_projection_provenance()?);
    let connection = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
    let installed: i64 = connection.query_row(
        "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    connection.execute(
        "UPDATE schema_version SET version = 0 WHERE component = 'graph_target_provenance'",
        [],
    )?;
    let pending = runtime
        .block_on(state.surreal_graph_for_health())
        .err()
        .expect("stale cached structural checkout rejected");
    assert!(matches!(
        pending,
        aether_mcp::AetherMcpError::Store(aether_store::StoreError::Compatibility(_))
    ));
    let server = AetherMcpServer::from_state(std::sync::Arc::new(state.clone()), false);
    let health = runtime.block_on(server.aether_health_logic(AetherHealthRequest {
        include: None,
        limit: None,
        min_risk: None,
    }))?;
    assert!(!health.analysis_available);
    assert_eq!(health.graph_coverage.status, "unavailable");
    assert_eq!(health.graph_coverage.call_edges, None);
    assert_eq!(health.graph_coverage.whole_program_completeness, "unknown");
    assert!(health.notes.iter().any(|note| note.contains("projection")));
    let score = runtime
        .block_on(
            server.aether_health_explain_logic(AetherHealthExplainRequest {
                crate_name: "unselected".to_owned(),
                semantic: Some(true),
            }),
        )
        .expect_err("pending projection must not silently produce a base-only semantic score");
    assert!(matches!(
        score,
        aether_mcp::AetherMcpError::Store(aether_store::StoreError::Compatibility(_))
    ));
    let independent = runtime.block_on(state.surreal_graph())?;
    assert!(std::sync::Arc::ptr_eq(&cached, &independent));
    connection.execute(
        "UPDATE schema_version SET version = ?1 WHERE component = 'graph_target_provenance'",
        [installed],
    )?;
    let current = runtime.block_on(state.surreal_graph_for_health())?;
    assert!(std::sync::Arc::ptr_eq(&cached, &current));
    Ok(())
}
