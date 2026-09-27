//! Dependency, usage-matrix and trait-split tools over the resolved symbol graph.

use super::*;

#[test]
fn mcp_dependencies_returns_callers_and_dependencies() -> Result<()> {
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
    assert!(!beta_edges.aggregated);
    assert_eq!(beta_edges.child_method_count, 0);
    assert_eq!(beta_edges.caller_count, 1);
    assert_eq!(beta_edges.callers.len(), 1);
    assert_eq!(beta_edges.callers[0].qualified_name, "alpha");
    assert_eq!(beta_edges.callers[0].methods_called, None);

    let alpha_edges = rt
        .block_on(
            server.aether_dependencies(Parameters(AetherDependenciesRequest {
                symbol_id: alpha_id.clone(),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(alpha_edges.found);
    assert!(!alpha_edges.aggregated);
    assert_eq!(alpha_edges.child_method_count, 0);
    assert_eq!(alpha_edges.dependency_count, 1);
    assert!(
        alpha_edges
            .dependencies
            .iter()
            .any(|edge| edge.qualified_name == "beta")
    );
    assert!(
        alpha_edges
            .dependencies
            .iter()
            .all(|edge| edge.referencing_methods.is_none())
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
fn mcp_usage_matrix_reports_consumers_clusters_and_uncalled_methods() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "mod consumer_a;\nmod consumer_b;\nmod consumer_c;\npub mod store;\n\npub use consumer_a::run_a;\npub use consumer_b::run_b;\npub use consumer_c::run_c;\n",
    )?;
    fs::write(
        workspace.join("src/store.rs"),
        "pub struct ExampleStore;\n\nimpl ExampleStore {\n    pub fn alpha(&self) -> i32 { 1 }\n    pub fn beta(&self) -> i32 { 2 }\n    pub fn gamma(&self) -> i32 { 3 }\n    pub fn delta(&self) -> i32 { 4 }\n}\n",
    )?;
    fs::write(
        workspace.join("src/consumer_a.rs"),
        "use crate::store::ExampleStore;\n\npub fn run_a() -> i32 {\n    let store = ExampleStore;\n    store.alpha() + store.beta()\n}\n",
    )?;
    fs::write(
        workspace.join("src/consumer_b.rs"),
        "use crate::store::ExampleStore;\n\npub fn run_b() -> i32 {\n    let store = ExampleStore;\n    store.alpha() + store.beta()\n}\n",
    )?;
    fs::write(
        workspace.join("src/consumer_c.rs"),
        "use crate::store::ExampleStore;\n\npub fn run_c() -> i32 {\n    let store = ExampleStore;\n    store.gamma()\n}\n",
    )?;

    run_index_and_seed_sir(workspace)?;

    let store = SqliteStore::open(workspace)?;
    let example_store_id = store
        .list_symbols_for_file("src/store.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "ExampleStore")
        .expect("ExampleStore symbol should exist")
        .id;
    let alpha_qualified_callers = store.get_callers("ExampleStore::alpha")?;
    assert!(alpha_qualified_callers.is_empty());

    let alpha_bare_callers = store.get_callers("alpha")?;
    assert_eq!(alpha_bare_callers.len(), 2);
    assert!(
        alpha_bare_callers
            .iter()
            .all(|edge| edge.target_qualified_name == "alpha")
    );

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_usage_matrix(Parameters(AetherUsageMatrixRequest {
                symbol: "ExampleStore".to_owned(),
                symbol_id: None,
                file: Some("src/store.rs".to_owned()),
                kind: Some("struct".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(response.target_file, "src/store.rs");
    assert_eq!(response.method_count, 4);
    assert_eq!(response.consumer_count, 3);
    assert_eq!(response.uncalled_methods, vec!["delta".to_owned()]);

    let matrix_by_file = response
        .matrix
        .iter()
        .map(|row| (row.consumer_file.clone(), row.methods_used.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        matrix_by_file.get("src/consumer_a.rs"),
        Some(&vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert_eq!(
        matrix_by_file.get("src/consumer_b.rs"),
        Some(&vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert_eq!(
        matrix_by_file.get("src/consumer_c.rs"),
        Some(&vec!["gamma".to_owned()])
    );

    let method_consumers = response
        .method_consumers
        .iter()
        .map(|row| (row.method.clone(), row.consumer_files.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        method_consumers.get("alpha"),
        Some(&vec![
            "src/consumer_a.rs".to_owned(),
            "src/consumer_b.rs".to_owned()
        ])
    );
    assert_eq!(
        method_consumers.get("beta"),
        Some(&vec![
            "src/consumer_a.rs".to_owned(),
            "src/consumer_b.rs".to_owned()
        ])
    );
    assert_eq!(
        method_consumers.get("gamma"),
        Some(&vec!["src/consumer_c.rs".to_owned()])
    );
    assert_eq!(method_consumers.get("delta"), Some(&Vec::new()));

    let cluster = response
        .suggested_clusters
        .iter()
        .find(|cluster| cluster.methods == vec!["alpha".to_owned(), "beta".to_owned()])
        .expect("alpha/beta cluster");
    assert_eq!(
        cluster.shared_consumers,
        vec![
            "src/consumer_a.rs".to_owned(),
            "src/consumer_b.rs".to_owned()
        ]
    );
    assert!(cluster.reason.contains("src/consumer_a.rs"));
    assert!(cluster.reason.contains("src/consumer_b.rs"));

    let by_id = rt
        .block_on(
            server.aether_usage_matrix(Parameters(AetherUsageMatrixRequest {
                symbol: String::new(),
                symbol_id: Some(example_store_id),
                file: None,
                kind: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(by_id.target, "ExampleStore");
    assert_eq!(by_id.target_file, "src/store.rs");
    assert_eq!(by_id.method_count, 4);

    Ok(())
}

#[test]
fn mcp_suggest_trait_split_returns_clusters() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    for symbol in [
        custom_symbol_record(
            "trait-example-store",
            "ExampleStore",
            "src/store.rs",
            "trait",
        ),
        custom_symbol_record(
            "trait-method-alpha",
            "ExampleStore::alpha",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "trait-method-beta",
            "ExampleStore::beta",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "trait-method-gamma",
            "ExampleStore::gamma",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "trait-method-delta",
            "ExampleStore::delta",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record("consumer-a", "run_a", "src/consumer_a.rs", "function"),
        custom_symbol_record("consumer-b", "run_b", "src/consumer_b.rs", "function"),
    ] {
        store.upsert_symbol(symbol)?;
    }

    store.upsert_edges(&[
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "alpha".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "beta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "gamma".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "delta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-alpha".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-alpha".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-beta".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-beta".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-gamma".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "trait-method-delta".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
    ])?;
    rebuild_symbol_neighbors(workspace)?;

    let trait_sir = SirAnnotation {
        intent: "Mock summary for ExampleStore".to_owned(),
        behavior: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
        edge_cases: None,
        complexity: None,
        method_dependencies: None,
    };
    store.write_sir_blob(
        "trait-example-store",
        serde_json::to_string(&trait_sir)?.as_str(),
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_suggest_trait_split(Parameters(AetherSuggestTraitSplitRequest {
                trait_name: "ExampleStore".to_owned(),
                file: Some("src/store.rs".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(response.message, None);
    assert_eq!(
        response.resolved_via.mode,
        AetherTraitSplitResolutionMode::Direct
    );
    assert_eq!(response.resolved_via.qualified_name, "ExampleStore");
    assert_eq!(response.resolved_via.kind, "trait");
    let suggestion = response.suggestion.expect("trait split suggestion");
    assert_eq!(suggestion.trait_name, "ExampleStore");
    assert_eq!(suggestion.trait_file, "src/store.rs");
    assert_eq!(suggestion.method_count, 4);
    assert_eq!(suggestion.suggested_traits.len(), 2);
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["delta".to_owned(), "gamma".to_owned()])
    );

    Ok(())
}

#[test]
fn mcp_suggest_trait_split_accepts_struct_targets() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    for symbol in [
        custom_symbol_record(
            "struct-example-store",
            "ExampleStore",
            "src/store.rs",
            "struct",
        ),
        custom_symbol_record(
            "struct-method-alpha",
            "ExampleStore::alpha",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "struct-method-beta",
            "ExampleStore::beta",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "struct-method-gamma",
            "ExampleStore::gamma",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record(
            "struct-method-delta",
            "ExampleStore::delta",
            "src/store.rs",
            "method",
        ),
        custom_symbol_record("consumer-a", "run_a", "src/consumer_a.rs", "function"),
        custom_symbol_record("consumer-b", "run_b", "src/consumer_b.rs", "function"),
    ] {
        store.upsert_symbol(symbol)?;
    }

    store.upsert_edges(&[
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "alpha".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "beta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "gamma".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "delta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-alpha".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-alpha".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-beta".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-beta".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-gamma".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "struct-method-delta".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/store.rs".to_owned(),
        },
    ])?;
    rebuild_symbol_neighbors(workspace)?;

    let struct_sir = SirAnnotation {
        intent: "Mock summary for ExampleStore".to_owned(),
        behavior: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
        edge_cases: None,
        complexity: None,
        method_dependencies: None,
    };
    store.write_sir_blob(
        "struct-example-store",
        serde_json::to_string(&struct_sir)?.as_str(),
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_suggest_trait_split(Parameters(AetherSuggestTraitSplitRequest {
                trait_name: "ExampleStore".to_owned(),
                file: Some("src/store.rs".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(response.message, None);
    assert_eq!(
        response.resolved_via.mode,
        AetherTraitSplitResolutionMode::Direct
    );
    assert_eq!(response.resolved_via.qualified_name, "ExampleStore");
    assert_eq!(response.resolved_via.kind, "struct");
    let suggestion = response.suggestion.expect("trait split suggestion");
    assert_eq!(suggestion.trait_name, "ExampleStore");
    assert_eq!(suggestion.trait_file, "src/store.rs");
    assert_eq!(suggestion.method_count, 4);
    assert_eq!(suggestion.suggested_traits.len(), 2);
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["delta".to_owned(), "gamma".to_owned()])
    );

    Ok(())
}

#[test]
fn mcp_suggest_trait_split_falls_back_to_same_file_implementor_methods() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    for symbol in [
        custom_symbol_record("trait-store", "Store", "src/lib.rs", "trait"),
        custom_symbol_record("struct-sqlite-store", "SqliteStore", "src/lib.rs", "struct"),
        custom_symbol_record(
            "sqlite-method-alpha",
            "SqliteStore::alpha",
            "src/lib.rs",
            "method",
        ),
        custom_symbol_record(
            "sqlite-method-beta",
            "SqliteStore::beta",
            "src/lib.rs",
            "method",
        ),
        custom_symbol_record(
            "sqlite-method-gamma",
            "SqliteStore::gamma",
            "src/lib.rs",
            "method",
        ),
        custom_symbol_record(
            "sqlite-method-delta",
            "SqliteStore::delta",
            "src/lib.rs",
            "method",
        ),
        custom_symbol_record("consumer-a", "run_a", "src/consumer_a.rs", "function"),
        custom_symbol_record("consumer-b", "run_b", "src/consumer_b.rs", "function"),
    ] {
        store.upsert_symbol(symbol)?;
    }

    store.upsert_edges(&[
        SymbolEdge {
            source_id: "struct-sqlite-store".to_owned(),
            target_qualified_name: "Store".to_owned(),
            edge_kind: EdgeKind::Implements,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "alpha".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-a".to_owned(),
            target_qualified_name: "beta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_a.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "gamma".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "consumer-b".to_owned(),
            target_qualified_name: "delta".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/consumer_b.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-alpha".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-alpha".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-beta".to_owned(),
            target_qualified_name: "SirMetaRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-beta".to_owned(),
            target_qualified_name: "SirBlob".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-gamma".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
        SymbolEdge {
            source_id: "sqlite-method-delta".to_owned(),
            target_qualified_name: "SymbolRecord".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        },
    ])?;
    rebuild_symbol_neighbors(workspace)?;

    let sqlite_sir = SirAnnotation {
        intent: "Mock summary for SqliteStore".to_owned(),
        behavior: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
        edge_cases: None,
        complexity: None,
        method_dependencies: None,
    };
    store.write_sir_blob(
        "struct-sqlite-store",
        serde_json::to_string(&sqlite_sir)?.as_str(),
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_suggest_trait_split(Parameters(AetherSuggestTraitSplitRequest {
                trait_name: "Store".to_owned(),
                file: Some("src/lib.rs".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(response.message, None);
    assert_eq!(
        response.resolved_via.mode,
        AetherTraitSplitResolutionMode::Implementor
    );
    assert_eq!(response.resolved_via.qualified_name, "SqliteStore");
    assert_eq!(response.resolved_via.kind, "struct");
    let suggestion = response.suggestion.expect("trait split suggestion");
    assert_eq!(suggestion.trait_name, "Store");
    assert_eq!(suggestion.trait_file, "src/lib.rs");
    assert_eq!(suggestion.method_count, 4);
    assert_eq!(suggestion.suggested_traits.len(), 2);
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["alpha".to_owned(), "beta".to_owned()])
    );
    assert!(
        suggestion
            .suggested_traits
            .iter()
            .any(|cluster| cluster.methods == vec!["delta".to_owned(), "gamma".to_owned()])
    );

    Ok(())
}

#[test]
fn mcp_usage_matrix_resolves_exact_qualified_name_before_fuzzy_search_limit() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "mod consumer;\npub mod store;\n\npub use consumer::run;\n",
    )?;

    let mut store_source = String::new();
    for index in 0..120 {
        store_source.push_str(&format!("pub struct AStore{index:03};\n"));
    }
    store_source
        .push_str("\npub struct Store;\n\nimpl Store {\n    pub fn alpha() -> i32 { 1 }\n}\n");
    fs::write(workspace.join("src/store.rs"), store_source)?;
    fs::write(
        workspace.join("src/consumer.rs"),
        "use crate::store::Store;\n\npub fn run() -> i32 {\n    Store::alpha()\n}\n",
    )?;

    run_index_and_seed_sir(workspace)?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_usage_matrix(Parameters(AetherUsageMatrixRequest {
                symbol: "Store".to_owned(),
                symbol_id: None,
                file: Some("src/store.rs".to_owned()),
                kind: Some("struct".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.target, "Store");
    assert_eq!(response.target_file, "src/store.rs");
    assert_eq!(response.method_count, 1);
    assert_eq!(response.consumer_count, 1);
    assert_eq!(response.matrix[0].consumer_file, "src/consumer.rs");
    assert_eq!(response.matrix[0].methods_used, vec!["alpha".to_owned()]);

    Ok(())
}

#[test]
fn mcp_dependencies_aggregate_type_level_call_relationships() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("src/lib.rs"),
        "pub struct Calc;\n\nimpl Calc {\n    pub fn alpha(&self) -> i32 { helper_one() }\n    pub fn beta(&self) -> i32 { helper_one() + helper_two() }\n    pub fn gamma(&self) -> i32 { helper_two() }\n}\n\npub fn helper_one() -> i32 { 1 }\npub fn helper_two() -> i32 { 2 }\n\npub fn run_x() -> i32 {\n    let calc = Calc;\n    calc.alpha() + calc.beta()\n}\n\npub fn run_y() -> i32 {\n    let calc = Calc;\n    calc.beta()\n}\n",
    )?;

    run_index_and_seed_sir(workspace)?;

    let store = SqliteStore::open(workspace)?;
    let calc_id = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "Calc")
        .expect("Calc symbol should exist")
        .id;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server
                .aether_dependencies(Parameters(AetherDependenciesRequest { symbol_id: calc_id })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert!(response.found);
    assert!(response.aggregated);
    assert_eq!(response.child_method_count, 3);
    assert_eq!(response.caller_count, 2);
    assert_eq!(response.dependency_count, 2);

    let callers = response
        .callers
        .iter()
        .map(|row| (row.qualified_name.clone(), row.methods_called))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(callers.get("run_x"), Some(&Some(2)));
    assert_eq!(callers.get("run_y"), Some(&Some(1)));

    let dependencies = response
        .dependencies
        .iter()
        .map(|row| (row.qualified_name.clone(), row.referencing_methods))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(dependencies.get("helper_one"), Some(&Some(2)));
    assert_eq!(dependencies.get("helper_two"), Some(&Some(2)));

    Ok(())
}
