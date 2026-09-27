//! Integration tests for the AETHER MCP tools. Shared fixtures live here; the tests are
//! grouped by tool family in the sibling modules.

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
    AetherAuditCandidatesRequest, AetherAuditReportRequest, AetherAuditResolveRequest,
    AetherAuditSubmitRequest, AetherBlastRadiusRequest, AetherCallChainRequest,
    AetherDependenciesRequest, AetherExplainRequest, AetherGetSirRequest,
    AetherHealthExplainRequest, AetherHealthHotspotsRequest, AetherHealthRequest, AetherMcpServer,
    AetherRecallRequest, AetherRefactorPrepRequest, AetherRememberRequest, AetherSearchRequest,
    AetherSirInjectRequest, AetherSuggestTraitSplitRequest, AetherSymbolLookupRequest,
    AetherSymbolTimelineRequest, AetherTestIntentsRequest, AetherTraitSplitResolutionMode,
    AetherUsageMatrixRequest, AetherVerifyIntentRequest, AetherWhyChangedReason,
    AetherWhyChangedRequest, AetherWhySelectorMode, AuditCategory, AuditCertainty, AuditSeverity,
    AuditStatus, MCP_SCHEMA_VERSION, MEMORY_SCHEMA_VERSION, SharedState, SirLevelRequest,
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

fn rebuild_symbol_neighbors(workspace: &Path) -> Result<()> {
    let conn = Connection::open(workspace.join(".aether/meta.sqlite"))?;
    conn.execute("DELETE FROM symbol_neighbors", [])?;
    conn.execute_batch(
        r#"
        INSERT OR REPLACE INTO symbol_neighbors (
            symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file
        )
        SELECT
            e.source_id,
            s_target.id,
            e.edge_kind,
            s_target.qualified_name,
            s_target.file_path
        FROM symbol_edges e
        JOIN symbols s_source ON s_source.id = e.source_id
        JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name;

        INSERT OR REPLACE INTO symbol_neighbors (
            symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file
        )
        SELECT
            s_target.id,
            e.source_id,
            CASE e.edge_kind
                WHEN 'calls' THEN 'called_by'
                WHEN 'depends_on' THEN 'depended_on_by'
                WHEN 'implements' THEN 'implemented_by'
                WHEN 'type_ref' THEN 'type_ref_by'
                ELSE e.edge_kind || '_reverse'
            END,
            s_source.qualified_name,
            s_source.file_path
        FROM symbol_edges e
        JOIN symbols s_source ON s_source.id = e.source_id
        JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name;
        "#,
    )?;
    Ok(())
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
        turbo_concurrency: None,
        dry_run: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        embeddings_only: false,
        inference_api_key_env: None,
        pause_flag: None,
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
            reasoning_trace: None,
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
            reasoning_trace: None,
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

/// Minimal Ollama-style embedding endpoint: answers every POST with a fixed vector.
fn spawn_stub_embedding_server() -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0u8; 8192];
            let _ = stream.read(&mut buffer);
            let body = r#"{"embedding":[0.1,0.2,0.3,0.4]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/api/embeddings"), handle)
}

fn seed_sir_with_pass(store: &SqliteStore, symbol_id: &str, generation_pass: &str) -> Result<()> {
    let sir_json = r#"{"intent":"seeded intent","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.6}"#;
    let hash = format!("hash-{symbol_id}");
    let history = store.record_sir_version_if_changed(
        symbol_id,
        hash.as_str(),
        "seed",
        "seed",
        sir_json,
        1_700_000_100,
        None,
    )?;
    store.write_sir_blob(symbol_id, sir_json)?;
    store.upsert_sir_meta(SirMetaRecord {
        id: symbol_id.to_owned(),
        sir_hash: hash,
        sir_version: history.version,
        provider: "seed".to_owned(),
        model: "seed".to_owned(),
        generation_pass: generation_pass.to_owned(),
        reasoning_trace: None,
        prompt_hash: None,
        staleness_score: None,
        updated_at: 1_700_000_100,
        sir_status: "fresh".to_owned(),
        last_error: None,
        last_attempt_at: 1_700_000_100,
    })?;
    Ok(())
}

mod core_tools;
mod graph_tools;
mod memory_audit;
mod refactor_verify;
mod search_history;
mod sir_inject;
