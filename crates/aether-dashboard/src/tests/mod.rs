use std::fs;
use std::sync::Arc;

use aether_config::{AetherConfig, GraphBackend, save_workspace_config};
use aether_core::{EdgeKind, SymbolEdge};
use aether_store::{
    DriftAnalysisStateRecord, DriftStore, SirFingerprintHistoryRecord, SirMetaRecord,
    SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore,
    TaskContextHistoryRecord,
};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use rusqlite::{Connection, params};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::{SharedState, dashboard_router};

mod api;
mod fragments;
mod language_family;
mod operations;

struct TestIds {
    primary: String,
}

async fn seeded_app() -> (TempDir, axum::Router, TestIds) {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);
    (
        temp,
        app,
        TestIds {
            primary: "sym-demo-run".to_owned(),
        },
    )
}

/// Id of a Python symbol that sorts after the seeded Rust `demo::` symbols.
const PYTHON_SYMBOL_ID: &str = "sym-demo-zz-script";

/// `seeded_app` plus one Python symbol whose name sorts last, so a lexical
/// search for "demo" returns it only after the three Rust symbols.
async fn seeded_app_with_python_symbol() -> (TempDir, axum::Router) {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    let store = SqliteStore::open(temp.path()).unwrap();
    store
        .upsert_symbol(SymbolRecord {
            id: PYTHON_SYMBOL_ID.to_owned(),
            file_path: "scripts/zz_script.py".to_owned(),
            language: "python".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "demo::zz_script".to_owned(),
            signature_fingerprint: "sig-zz-script".to_owned(),
            last_seen_at: 1_700_000_020,
        })
        .unwrap();
    drop(store);
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);
    (temp, app)
}

async fn empty_app() -> (TempDir, axum::Router) {
    let temp = TempDir::new().unwrap();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).unwrap();
    let _store = SqliteStore::open(temp.path()).unwrap();
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);
    (temp, app)
}

/// Sends one request (with an optional JSON body) and returns the status and parsed JSON body.
async fn send_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(body.map_or_else(Body::empty, |raw| Body::from(raw.to_owned())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

fn seed_workspace(workspace: &std::path::Path) {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(workspace, &config).unwrap();

    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = \"dashboard-test\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\nmembers = [\".\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn run_demo() -> i32 {\n    helper()\n}\n\nfn helper() -> i32 {\n    1\n}\n",
    )
    .unwrap();
    fs::write(
        workspace.join("src/main.rs"),
        "fn main() {\n    let _ = dashboard_test::run_demo();\n}\n",
    )
    .unwrap();

    let store = SqliteStore::open(workspace).unwrap();

    let run_symbol = SymbolRecord {
        id: "sym-demo-run".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "demo::run".to_owned(),
        signature_fingerprint: "sig-run".to_owned(),
        last_seen_at: 1_700_000_000,
    };
    let helper_symbol = SymbolRecord {
        id: "sym-demo-helper".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "demo::helper".to_owned(),
        signature_fingerprint: "sig-helper".to_owned(),
        last_seen_at: 1_700_000_005,
    };
    let caller_symbol = SymbolRecord {
        id: "sym-demo-main".to_owned(),
        file_path: "src/main.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "demo::main".to_owned(),
        signature_fingerprint: "sig-main".to_owned(),
        last_seen_at: 1_700_000_010,
    };

    store.upsert_symbol(run_symbol).unwrap();
    store.upsert_symbol(helper_symbol).unwrap();
    store.upsert_symbol(caller_symbol).unwrap();

    store
        .upsert_edges(&[
            SymbolEdge {
                source_id: "sym-demo-run".to_owned(),
                target_qualified_name: "demo::helper".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            SymbolEdge {
                source_id: "sym-demo-main".to_owned(),
                target_qualified_name: "demo::run".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/main.rs".to_owned(),
            },
        ])
        .unwrap();
    let conn = Connection::open(workspace.join(".aether/meta.sqlite")).unwrap();
    conn.execute(
        r#"
        INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            "sym-demo-run",
            "sym-demo-helper",
            "calls",
            "demo::helper",
            "src/lib.rs"
        ],
    )
    .unwrap();
    conn.execute(
        r#"
        INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            "sym-demo-helper",
            "sym-demo-run",
            "called_by",
            "demo::run",
            "src/lib.rs"
        ],
    )
    .unwrap();
    conn.execute(
        r#"
        INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            "sym-demo-main",
            "sym-demo-run",
            "calls",
            "demo::run",
            "src/lib.rs"
        ],
    )
    .unwrap();
    conn.execute(
        r#"
        INSERT INTO symbol_neighbors (symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            "sym-demo-run",
            "sym-demo-main",
            "called_by",
            "demo::main",
            "src/main.rs"
        ],
    )
    .unwrap();

    store
        .upsert_sir_meta(SirMetaRecord {
            id: "sym-demo-run".to_owned(),
            sir_hash: "hash-demo-run".to_owned(),
            sir_version: 1,
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            generation_pass: "single".to_owned(),
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: 1_700_000_100,
            sir_status: "ready".to_owned(),
            last_error: None,
            last_attempt_at: 1_700_000_100,
        })
        .unwrap();
    store
        .write_sir_blob(
            "sym-demo-run",
            r#"{"intent":"Run demo task","purpose":"Execute demo path","inputs":["ctx"],"outputs":["ok"]}"#,
        )
        .unwrap();

    store
        .upsert_drift_analysis_state(DriftAnalysisStateRecord {
            last_analysis_commit: Some("abc123".to_owned()),
            last_analysis_at: Some(1_700_000_200),
            symbols_analyzed: 3,
            drift_detected: 1,
        })
        .unwrap();

    // Seed task context history for task-context page tests
    store
        .insert_task_context_history(&TaskContextHistoryRecord {
            task_description: "Fix the login bug in auth module".to_owned(),
            branch_name: Some("fix/auth-login".to_owned()),
            resolved_symbol_ids: "sym-demo-run,sym-demo-helper".to_owned(),
            resolved_file_paths: "src/lib.rs,src/main.rs".to_owned(),
            total_symbols: 2,
            budget_used: 8000,
            budget_max: 32000,
            created_at: 1_700_000_300,
        })
        .unwrap();

    // Seed continuous status JSON for batch/continuous page tests
    let continuous_dir = aether_config::aether_dir(workspace).join("continuous");
    fs::create_dir_all(&continuous_dir).unwrap();
    fs::write(
        continuous_dir.join("status.json"),
        r#"{
            "last_started_at": 1700000100,
            "last_completed_at": 1700000200,
            "last_successful_completed_at": 1700000200,
            "total_symbols": 3,
            "symbols_with_sir": 1,
            "scored_symbols": 3,
            "score_bands": { "critical": 0, "high": 1, "medium": 1, "low": 1 },
            "most_stale_symbol": { "symbol_id": "sym-demo-run", "qualified_name": "demo::run", "staleness_score": 0.85 },
            "selected_symbols": 1,
            "written_requests": 2,
            "skipped_requests": 1,
            "unresolved_symbols": 0,
            "chunk_count": 1,
            "auto_submit": false,
            "submitted_chunks": 0,
            "ingested_results": 0,
            "fingerprint_rows": 0,
            "requeue_pass": "scan",
            "last_error": null
        }"#,
    )
    .unwrap();

    // Seed fingerprint history for fingerprint page tests
    store
        .insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: "sym-demo-run".to_owned(),
            timestamp: 1_700_000_400,
            prompt_hash: "abc123def456".to_owned(),
            prompt_hash_previous: None,
            trigger: "initial".to_owned(),
            source_changed: true,
            neighbor_changed: false,
            config_changed: false,
            generation_model: Some("gemini-flash".to_owned()),
            generation_pass: Some("scan".to_owned()),
            delta_sem: Some(0.42),
            sir_write_generation: None,
        })
        .unwrap();
    store
        .insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: "sym-demo-run".to_owned(),
            timestamp: 1_700_000_500,
            prompt_hash: "def789ghi012".to_owned(),
            prompt_hash_previous: Some("abc123def456".to_owned()),
            trigger: "neighbor_change".to_owned(),
            source_changed: false,
            neighbor_changed: true,
            config_changed: false,
            generation_model: Some("gemini-flash".to_owned()),
            generation_pass: Some("triage".to_owned()),
            delta_sem: Some(0.15),
            sir_write_generation: None,
        })
        .unwrap();
}
