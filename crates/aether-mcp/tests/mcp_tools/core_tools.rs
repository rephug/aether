//! Tool handler round trips against a local store, `aether_get_sir` levels and the health tools.

use super::*;

#[test]
fn mcp_tool_handlers_work_with_local_store() -> Result<()> {
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
enabled = false
provider = "qwen3_local"
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

    run_index_and_seed_sir(workspace)?;

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
    let health = rt
        .block_on(server.aether_health(Parameters(AetherHealthRequest {
            include: None,
            limit: Some(10),
            min_risk: Some(0.0),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(health.schema_version, "1.0");
    assert!(health.analysis.analyzed_at > 0);
    assert!(health.critical_symbols.len() <= 10);
    let lookup = rt
        .block_on(
            server.aether_symbol_lookup(Parameters(AetherSymbolLookupRequest {
                query: "alpha".to_owned(),
                limit: None,
                symbol_ids: None,
                include_source: None,
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
            query: "alpha".to_owned(),
            limit: Some(10),
            mode: Some(SearchMode::Semantic),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(semantic_search.mode_requested, SearchMode::Semantic);
    assert_eq!(semantic_search.mode_used, SearchMode::Lexical);
    assert_eq!(
        semantic_search.fallback_reason.as_deref(),
        Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED)
    );
    assert!(!semantic_search.matches.is_empty());
    assert_eq!(
        semantic_search.result_count as usize,
        semantic_search.matches.len()
    );
    assert!(
        semantic_search
            .matches
            .iter()
            .all(|row| row.semantic_score.is_none())
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
    assert_eq!(sir_annotation.method_dependencies, None);
    assert_eq!(sir.sir_status.as_deref(), Some("fresh"));
    assert_eq!(sir.last_error, None);
    assert!(sir.last_attempt_at.unwrap_or_default() > 0);
    assert!(symbol_access_count(workspace, &explain.symbol_id)? >= 2);

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
        generation_pass: existing_meta.generation_pass.clone(),
        reasoning_trace: existing_meta.reasoning_trace.clone(),
        prompt_hash: None,
        staleness_score: None,
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
fn mcp_get_sir_returns_method_dependencies_when_present() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);
    fs::create_dir_all(workspace.join("src"))?;
    fs::write(workspace.join("src/lib.rs"), "fn alpha() -> i32 { 1 }\n")?;

    run_index_and_seed_sir(workspace)?;

    let store = SqliteStore::open(workspace)?;
    let symbol = store
        .list_symbols_for_file("src/lib.rs")?
        .into_iter()
        .find(|symbol| symbol.qualified_name == "alpha")
        .expect("alpha symbol should exist");

    let sir = SirAnnotation {
        intent: "Mock summary for alpha".to_owned(),
        behavior: None,
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: vec!["StoreError".to_owned(), "SymbolRecord".to_owned()],
        error_modes: Vec::new(),
        confidence: 0.9,
        edge_cases: None,
        complexity: None,
        method_dependencies: Some(HashMap::from([(
            "load".to_owned(),
            vec!["StoreError".to_owned(), "SymbolRecord".to_owned()],
        )])),
    };
    store.write_sir_blob(symbol.id.as_str(), serde_json::to_string(&sir)?.as_str())?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(server.aether_get_sir(Parameters(AetherGetSirRequest {
            level: None,
            symbol_id: Some(symbol.id.clone()),
            file_path: None,
            module_path: None,
            language: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    let method_dependencies = response
        .sir
        .expect("sir should be present")
        .method_dependencies
        .expect("method dependencies should be present");
    assert_eq!(
        method_dependencies.get("load"),
        Some(&vec!["StoreError".to_owned(), "SymbolRecord".to_owned()])
    );
    assert!(response.sir_json.contains("\"method_dependencies\""));

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
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    fs::create_dir_all(workspace.join("src/moda"))?;
    fs::write(workspace.join("src/moda/a.rs"), "fn alpha() -> i32 { 1 }\n")?;
    fs::write(workspace.join("src/moda/b.rs"), "fn beta() -> i32 { 2 }\n")?;

    run_index_and_seed_sir(workspace)?;

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
fn mcp_health_hotspots_tool() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    seed_health_workspace(workspace, GraphBackend::Sqlite)?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let output = rt
        .block_on(
            server.aether_health_hotspots(Parameters(AetherHealthHotspotsRequest {
                limit: Some(5),
                max_score: Some(100),
                semantic: Some(false),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0
        .text;

    assert!(output.contains("Workspace Health:"));
    assert!(output.contains("mcp-health-test"));
    assert!(output.matches("mcp-health-test - ").count() <= 1);

    Ok(())
}

#[test]
fn mcp_health_explain_tool() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    seed_health_workspace(workspace, GraphBackend::Surreal)?;

    let store = SqliteStore::open(workspace)?;
    let symbols = vec![
        health_symbol("sym-sir-a", "crate::sir_alpha"),
        health_symbol("sym-sir-b", "crate::sir_beta"),
        health_symbol("sym-sir-c", "crate::sir_gamma"),
        health_symbol("sym-sir-d", "crate::sir_delta"),
        health_symbol("sym-note-a", "crate::note_alpha"),
        health_symbol("sym-note-b", "crate::note_beta"),
        health_symbol("sym-note-c", "crate::note_gamma"),
        health_symbol("sym-note-d", "crate::note_delta"),
    ];
    for symbol in &symbols {
        store.upsert_symbol(symbol.clone())?;
    }
    store.upsert_symbol_embedding(embedding_record("sym-sir-a", vec![1.0, 0.0]))?;
    store.upsert_symbol_embedding(embedding_record("sym-sir-b", vec![0.95, 0.05]))?;
    store.upsert_symbol_embedding(embedding_record("sym-sir-c", vec![0.92, 0.08]))?;
    store.upsert_symbol_embedding(embedding_record("sym-sir-d", vec![0.9, 0.1]))?;
    store.upsert_symbol_embedding(embedding_record("sym-note-a", vec![0.0, 1.0]))?;
    store.upsert_symbol_embedding(embedding_record("sym-note-b", vec![0.05, 0.95]))?;
    store.upsert_symbol_embedding(embedding_record("sym-note-c", vec![0.08, 0.92]))?;
    store.upsert_symbol_embedding(embedding_record("sym-note-d", vec![0.1, 0.9]))?;
    store.replace_community_snapshot(
        "snapshot-1",
        now_millis(),
        &[
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-sir-a".to_owned(),
                community_id: 1,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-sir-b".to_owned(),
                community_id: 1,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-sir-c".to_owned(),
                community_id: 1,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-sir-d".to_owned(),
                community_id: 1,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-note-a".to_owned(),
                community_id: 2,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-note-b".to_owned(),
                community_id: 2,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-note-c".to_owned(),
                community_id: 2,
                captured_at: now_millis(),
            },
            CommunitySnapshotRecord {
                snapshot_id: "snapshot-1".to_owned(),
                symbol_id: "sym-note-d".to_owned(),
                community_id: 2,
                captured_at: now_millis(),
            },
        ],
    )?;
    store.replace_test_intents_for_file(
        "tests/health_test.rs",
        &[TestIntentRecord {
            intent_id: "intent-sir".to_owned(),
            file_path: "tests/health_test.rs".to_owned(),
            test_name: "test_sir_alpha".to_owned(),
            intent_text: "covers sir alpha behavior".to_owned(),
            group_label: None,
            language: "rust".to_owned(),
            symbol_id: Some("sym-sir-a".to_owned()),
            created_at: now_millis(),
            updated_at: now_millis(),
        }],
    )?;

    let rt = Runtime::new()?;
    rt.block_on(async {
        let graph = SurrealGraphStore::open(workspace).await?;
        for symbol in &symbols {
            graph.upsert_symbol_node(symbol).await?;
        }
        for (source_id, target_id) in [
            ("sym-sir-a", "sym-sir-b"),
            ("sym-sir-b", "sym-sir-c"),
            ("sym-sir-c", "sym-sir-d"),
            ("sym-note-a", "sym-note-b"),
            ("sym-note-b", "sym-note-c"),
            ("sym-note-c", "sym-note-d"),
        ] {
            graph
                .upsert_edge(&ResolvedEdge {
                    source_id: source_id.to_owned(),
                    target_id: target_id.to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                })
                .await?;
        }
        Ok::<(), anyhow::Error>(())
    })?;

    let server = AetherMcpServer::new(workspace, false)?;
    let output = rt
        .block_on(
            server.aether_health_explain(Parameters(AetherHealthExplainRequest {
                crate_name: "mcp-health-test".to_owned(),
                semantic: Some(true),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0
        .text;

    assert!(output.contains("Health Score: mcp-health-test"));
    assert!(output.contains("Violations:"));
    assert!(output.contains("Semantic signals:"));
    assert!(output.contains("Split suggestion:"));
    assert!(output.contains("sir_ops"));

    Ok(())
}
