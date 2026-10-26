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
        "fn leaf() -> i32 { 1 }\nfn helper() -> i32 { 2 }\nfn uses() -> i32 { helper() + leaf() }\nfn external() -> String { String::new() }\nfn calls_dup() -> i32 { dup() }\n",
    )?;
    fs::write(workspace.join("src/a.rs"), "fn dup() -> i32 { 1 }\n")?;
    fs::write(workspace.join("src/b.rs"), "fn dup() -> i32 { 2 }\n")?;
    // `s.area()` is recorded by its bare method name, so attributing it is a guess.
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

    // Calls nothing: the empty list is complete.
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
        "1 match no qualified name exactly",
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

    // A caller found through a bare method name is listed, and labelled a heuristic match.
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
        (1, 1)
    );
    assert!(
        area.coverage_note.contains("1 of those by heuristic"),
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

    // A name two files declare: both candidates listed, and counted as ambiguous.
    let calls_dup = dependencies(id_of("src/lib.rs", "calls_dup")?)?;
    assert_eq!(calls_dup.dependency_count, 2);
    assert_eq!(calls_dup.dependency_coverage.ambiguous, 1);

    // ...and neither `dup` gets the caller, which coverage says.
    let dup = dependencies(id_of("src/a.rs", "dup")?)?;
    assert!(dup.callers.is_empty());
    assert_eq!(
        (
            dup.caller_coverage.name_matches,
            dup.caller_coverage.ambiguous
        ),
        (1, 1)
    );
    assert!(
        dup.coverage_note.contains("could be calling it"),
        "{}",
        dup.coverage_note
    );

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
            .contains("None of the recorded calls reach a workspace symbol."),
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
        (5, 2, 1, 2)
    );
    assert!(status.call_graph.rule.contains("exact qualified name"));
    assert!(status.call_graph.rule.contains("may be internal"));
    assert!(status.call_graph.rule.contains("macro arguments"));

    Ok(())
}
