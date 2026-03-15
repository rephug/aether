use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{
    AetherConfig, EmbeddingProviderKind, EmbeddingVectorBackend, GraphBackend,
    save_workspace_config,
};
use aether_core::{
    EdgeKind, SEARCH_FALLBACK_EMBEDDINGS_DISABLED, SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY,
    SearchMode, SymbolEdge,
};
use aether_mcp::{
    AetherBlastRadiusRequest, AetherCallChainRequest, AetherDependenciesRequest,
    AetherExplainRequest, AetherGetSirRequest, AetherHealthExplainRequest,
    AetherHealthHotspotsRequest, AetherHealthRequest, AetherMcpServer, AetherRecallRequest,
    AetherRefactorPrepRequest, AetherRememberRequest, AetherSearchRequest,
    AetherSuggestTraitSplitRequest, AetherSymbolLookupRequest, AetherSymbolTimelineRequest,
    AetherTestIntentsRequest, AetherTraitSplitResolutionMode, AetherUsageMatrixRequest,
    AetherVerifyIntentRequest, AetherWhyChangedReason, AetherWhyChangedRequest,
    AetherWhySelectorMode, MCP_SCHEMA_VERSION, MEMORY_SCHEMA_VERSION, SharedState, SirLevelRequest,
};
#[cfg(feature = "verification")]
use aether_mcp::{AetherVerifyMode, AetherVerifyRequest};
use aether_sir::{
    FileSir, SirAnnotation, file_sir_hash, sir_hash, synthetic_file_sir_id, synthetic_module_sir_id,
};
use aether_store::{
    CommunitySnapshotRecord, DriftStore, GraphStore, ProjectNoteStore, ResolvedEdge,
    SemanticIndexStore, SirHistoryStore, SirMetaRecord, SirStateStore, SqliteStore,
    SurrealGraphStore, SymbolCatalogStore, SymbolEmbeddingRecord, SymbolRecord,
    SymbolRelationStore, TestIntentRecord, TestIntentStore,
};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;
use tempfile::tempdir;
use tokio::runtime::Runtime;

fn symbol_access_count(workspace: &Path, symbol_id: &str) -> Result<i64> {
    let conn = Connection::open(workspace.join(".aether/meta.sqlite"))?;
    let count = conn.query_row(
        "SELECT access_count FROM symbols WHERE id = ?1",
        rusqlite::params![symbol_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count)
}

fn write_test_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
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
    )
    .expect("write config");
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn unique_env_name(prefix: &str) -> String {
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    )
}

fn seed_health_workspace(workspace: &Path, graph_backend: GraphBackend) -> Result<()> {
    let mut config = AetherConfig::default();
    config.embeddings.enabled = true;
    config.embeddings.provider = EmbeddingProviderKind::Qwen3Local;
    config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
    config.embeddings.model = Some("qwen3-embeddings-4B".to_owned());
    config.storage.graph_backend = graph_backend;
    config.health_score.file_loc_warn = 1;
    config.health_score.file_loc_fail = 2;
    config.health_score.trait_method_warn = 1;
    config.health_score.trait_method_fail = 2;
    save_workspace_config(workspace, &config)?;

    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"mcp-health-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\nmembers = [\".\"]\nresolver = \"2\"\n",
    )?;
    fs::write(
        workspace.join("src/lib.rs"),
        "pub trait Store {\n    fn alpha(&self);\n    fn beta(&self);\n    fn gamma(&self);\n}\n\npub fn sir_alpha() -> i32 { 1 }\npub fn sir_beta() -> i32 { sir_alpha() }\npub fn sir_gamma() -> i32 { sir_beta() }\npub fn sir_delta() -> i32 { sir_gamma() }\npub fn note_alpha() -> i32 { 3 }\npub fn note_beta() -> i32 { note_alpha() }\npub fn note_gamma() -> i32 { note_beta() }\npub fn note_delta() -> i32 { note_gamma() }\n",
    )?;
    Ok(())
}

fn health_symbol(id: &str, qualified_name: &str) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: now_millis(),
    }
}

fn embedding_record(symbol_id: &str, embedding: Vec<f32>) -> SymbolEmbeddingRecord {
    SymbolEmbeddingRecord {
        symbol_id: symbol_id.to_owned(),
        sir_hash: format!("sir-{symbol_id}"),
        provider: "qwen3_local".to_owned(),
        model: "qwen3-embeddings-4B".to_owned(),
        embedding,
        updated_at: now_millis(),
    }
}

fn custom_symbol_record(
    id: &str,
    qualified_name: &str,
    file_path: &str,
    kind: &str,
) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: file_path.to_owned(),
        language: "rust".to_owned(),
        kind: kind.to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: now_millis(),
    }
}

fn run_index_and_seed_sir(workspace: &Path) -> Result<()> {
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
        dry_run: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        embeddings_only: false,
        inference_api_key_env: None,
    })?;

    let store = SqliteStore::open(workspace)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let mut file_entries = std::collections::HashMap::<(String, String), Vec<String>>::new();
    let mut file_exports = std::collections::HashMap::<(String, String), Vec<String>>::new();

    for symbol_id in store.list_all_symbol_ids()? {
        let Some(symbol) = store.get_symbol_record(symbol_id.as_str())? else {
            continue;
        };
        let symbol_name = symbol
            .qualified_name
            .rsplit("::")
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or(symbol.qualified_name.as_str());
        let sir = SirAnnotation {
            intent: format!("Mock summary for {symbol_name}"),
            inputs: Vec::new(),
            outputs: Vec::new(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.9,
            method_dependencies: None,
        };
        let sir_json = serde_json::to_string(&sir)?;
        let hash = sir_hash(&sir);
        store.write_sir_blob(symbol.id.as_str(), sir_json.as_str())?;
        store.upsert_sir_meta(SirMetaRecord {
            id: symbol.id.clone(),
            sir_hash: hash,
            sir_version: 1,
            provider: "test".to_owned(),
            model: "test".to_owned(),
            generation_pass: "single".to_owned(),
            prompt_hash: None,
            staleness_score: None,
            updated_at: now,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: now,
        })?;
        let file_key = (symbol.language.clone(), symbol.file_path.clone());
        file_entries
            .entry(file_key.clone())
            .or_default()
            .push(sir.intent.clone());
        file_exports
            .entry(file_key)
            .or_default()
            .push(symbol.qualified_name.clone());
    }

    for ((language, file_path), intents) in file_entries {
        let exports = file_exports
            .remove(&(language.clone(), file_path.clone()))
            .unwrap_or_default();
        let file_sir = FileSir {
            intent: intents.join("; "),
            exports: exports.clone(),
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            symbol_count: intents.len(),
            confidence: 0.9,
        };
        let file_rollup_id = synthetic_file_sir_id(language.as_str(), file_path.as_str());
        let file_json = serde_json::to_string(&file_sir)?;
        store.write_sir_blob(file_rollup_id.as_str(), file_json.as_str())?;
        store.upsert_sir_meta(SirMetaRecord {
            id: file_rollup_id,
            sir_hash: file_sir_hash(&file_sir),
            sir_version: 1,
            provider: "test".to_owned(),
            model: "test".to_owned(),
            generation_pass: "single".to_owned(),
            prompt_hash: None,
            staleness_score: None,
            updated_at: now,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: now,
        })?;
    }
    Ok(())
}

fn mark_leaf_sir_deep(workspace: &Path) -> Result<()> {
    let store = SqliteStore::open(workspace)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    for symbol_id in store.list_all_symbol_ids()? {
        let Some(mut meta) = store.get_sir_meta(symbol_id.as_str())? else {
            continue;
        };
        meta.generation_pass = "deep".to_owned();
        meta.updated_at = now;
        meta.last_attempt_at = now;
        store.upsert_sir_meta(meta)?;
    }
    Ok(())
}

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
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: vec!["StoreError".to_owned(), "SymbolRecord".to_owned()],
        error_modes: Vec::new(),
        confidence: 0.9,
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
        dry_run: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        embeddings_only: false,
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

    let trait_sir = SirAnnotation {
        intent: "Mock summary for ExampleStore".to_owned(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
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

    let struct_sir = SirAnnotation {
        intent: "Mock summary for ExampleStore".to_owned(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
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

    let sqlite_sir = SirAnnotation {
        intent: "Mock summary for SqliteStore".to_owned(),
        inputs: Vec::new(),
        outputs: Vec::new(),
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
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
        dry_run: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        embeddings_only: false,
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

#[test]
fn mcp_refactor_prep_and_verify_intent_tools_round_trip() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    write_test_config(workspace);
    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("Cargo.toml"),
        r#"[package]
name = "mcp-refactor-test"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["."]
resolver = "2"
"#,
    )?;
    fs::write(
        workspace.join("src/lib.rs"),
        r#"pub fn alpha() -> i32 { 1 }
pub fn beta() -> i32 { alpha() }
"#,
    )?;
    run_index_and_seed_sir(workspace)?;
    mark_leaf_sir_deep(workspace)?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let prep = rt
        .block_on(
            server.aether_refactor_prep(Parameters(AetherRefactorPrepRequest {
                file: Some("src/lib.rs".to_owned()),
                crate_name: None,
                top_n: Some(2),
                local: Some(false),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(prep.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(prep.scope, "file:src/lib.rs");
    let snapshot_id = prep.snapshot_id.clone();
    assert_eq!(prep.deep_failed, 0);

    let verify = rt
        .block_on(
            server.aether_verify_intent(Parameters(AetherVerifyIntentRequest {
                snapshot: snapshot_id,
                threshold: Some(0.85),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(verify.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(verify.scope, "file:src/lib.rs");
    assert!(verify.passed);
    assert_eq!(verify.failed_entries, 0);
    Ok(())
}

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_runs_allowlisted_subset_and_has_stable_response_shape() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
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

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_reports_failure_status_output_and_allowlist_rejection() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
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

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_handles_unavailable_container_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
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

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_handles_unavailable_microvm_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(workspace.join("vmlinux"), "")?;
    fs::write(workspace.join("rootfs.ext4"), "")?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
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

#[test]
fn mcp_memory_tools_dedup_and_recall_fallback_work() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let first = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "We selected sqlite for deterministic local persistence.".to_owned(),
            tags: Some(vec!["architecture".to_owned()]),
            entity_refs: None,
            file_refs: Some(vec!["crates/aether-store/src/lib.rs".to_owned()]),
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(first.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(first.action, "created");
    assert_eq!(first.tags, vec!["architecture".to_owned()]);

    let second = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "We selected sqlite for deterministic local persistence.".to_owned(),
            tags: Some(vec!["database".to_owned()]),
            entity_refs: None,
            file_refs: None,
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(second.note_id, first.note_id);
    assert_eq!(second.action, "updated_existing");
    assert_eq!(
        second.tags,
        vec!["architecture".to_owned(), "database".to_owned()]
    );

    let recall = rt
        .block_on(server.aether_recall(Parameters(AetherRecallRequest {
            query: "why sqlite".to_owned(),
            mode: Some(SearchMode::Semantic),
            limit: Some(5),
            include_archived: Some(false),
            tags_filter: Some(vec!["architecture".to_owned()]),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(recall.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(recall.mode_requested, SearchMode::Semantic);
    assert_eq!(recall.mode_used, SearchMode::Lexical);
    assert_eq!(
        recall.fallback_reason.as_deref(),
        Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED)
    );
    assert_eq!(recall.result_count, 1);
    assert_eq!(recall.notes[0].note_id, first.note_id);
    assert_eq!(
        recall.notes[0].tags,
        vec!["architecture".to_owned(), "database".to_owned()]
    );

    let session_note = rt
        .block_on(
            server.aether_session_note(Parameters(AetherRememberRequest {
                content: "Refactoring payment flow to reduce batch memory usage.".to_owned(),
                tags: Some(vec!["session".to_owned(), "refactor".to_owned()]),
                entity_refs: None,
                file_refs: Some(vec!["src/payments/processor.rs".to_owned()]),
                symbol_refs: Some(vec!["sym-payment".to_owned()]),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(session_note.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(session_note.action, "created");
    assert_eq!(session_note.source_type, "session");

    let store = SqliteStore::open(workspace)?;
    let stored_session_note = store
        .get_project_note(session_note.note_id.as_str())?
        .expect("session note should be persisted");
    assert_eq!(stored_session_note.source_type, "session");

    Ok(())
}

#[test]
fn mcp_memory_tool_response_schema_shapes_are_stable() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);
    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let remember = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "Design rationale note".to_owned(),
            tags: Some(vec!["design".to_owned()]),
            entity_refs: None,
            file_refs: None,
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let remember_json = serde_json::to_value(&remember)?;
    let remember_obj = remember_json
        .as_object()
        .expect("remember response should serialize as object");
    for key in [
        "schema_version",
        "note_id",
        "action",
        "content_hash",
        "tags",
        "created_at",
    ] {
        assert!(remember_obj.contains_key(key), "missing key: {key}");
    }

    let session_note = rt
        .block_on(
            server.aether_session_note(Parameters(AetherRememberRequest {
                content: "Session note content".to_owned(),
                tags: Some(vec!["session".to_owned()]),
                entity_refs: None,
                file_refs: None,
                symbol_refs: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let session_note_json = serde_json::to_value(&session_note)?;
    let session_note_obj = session_note_json
        .as_object()
        .expect("session note response should serialize as object");
    for key in ["schema_version", "note_id", "action", "source_type"] {
        assert!(session_note_obj.contains_key(key), "missing key: {key}");
    }

    let recall = rt
        .block_on(server.aether_recall(Parameters(AetherRecallRequest {
            query: "design".to_owned(),
            mode: Some(SearchMode::Lexical),
            limit: Some(5),
            include_archived: Some(false),
            tags_filter: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let recall_json = serde_json::to_value(&recall)?;
    let recall_obj = recall_json
        .as_object()
        .expect("recall response should serialize as object");
    for key in [
        "schema_version",
        "query",
        "mode_requested",
        "mode_used",
        "fallback_reason",
        "result_count",
        "notes",
    ] {
        assert!(recall_obj.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_blast_radius_response_schema_shape_is_stable() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_blast_radius(Parameters(AetherBlastRadiusRequest {
                file: "src/lib.rs".to_owned(),
                min_risk: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    let json = serde_json::to_value(&response)?;
    let object = json
        .as_object()
        .expect("blast radius response should serialize as object");
    for key in [
        "schema_version",
        "target_file",
        "mining_state",
        "coupled_files",
        "test_guards",
    ] {
        assert!(object.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_test_intents_tool_and_blast_radius_return_test_guards() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;
    fs::create_dir_all(workspace.join("src"))?;
    fs::create_dir_all(workspace.join("tests"))?;
    fs::write(workspace.join("src/payment.rs"), "fn charge() {}\n")?;
    fs::write(
        workspace.join("tests/payment_test.rs"),
        "#[test]\nfn test_charge() {}\n",
    )?;

    let store = SqliteStore::open(workspace)?;
    store.replace_test_intents_for_file(
        "tests/payment_test.rs",
        &[
            TestIntentRecord {
                intent_id: "intent-1".to_owned(),
                file_path: "tests/payment_test.rs".to_owned(),
                test_name: "test_charge".to_owned(),
                intent_text: "charges correctly".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: None,
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
            },
            TestIntentRecord {
                intent_id: "intent-2".to_owned(),
                file_path: "tests/payment_test.rs".to_owned(),
                test_name: "test_errors".to_owned(),
                intent_text: "handles invalid input".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: None,
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
            },
        ],
    )?;

    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let intents = rt
        .block_on(
            server.aether_test_intents(Parameters(AetherTestIntentsRequest {
                file: Some("tests/payment_test.rs".to_owned()),
                symbol_id: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(intents.result_count, 2);
    assert!(
        intents
            .intents
            .iter()
            .any(|entry| entry.intent_text == "charges correctly")
    );

    let blast = rt
        .block_on(
            server.aether_blast_radius(Parameters(AetherBlastRadiusRequest {
                file: "src/payment.rs".to_owned(),
                min_risk: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!blast.test_guards.is_empty());
    assert_eq!(blast.test_guards[0].test_file, "tests/payment_test.rs");
    assert!(
        blast.test_guards[0]
            .intents
            .contains(&"charges correctly".to_owned())
    );
    assert_eq!(blast.test_guards[0].inference_method, "naming_convention");

    Ok(())
}
