use std::sync::Arc;

use aether_config::{AetherConfig, GraphBackend, save_workspace_config};
use aether_dashboard::{SharedState, dashboard_router};
use aether_store::{SqliteStore, Store, SymbolRecord};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn dashboard_router_serves_shell_api_and_fragments() {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);

    let shell = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shell.status(), StatusCode::OK);

    let overview = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(overview.status(), StatusCode::OK);
    let overview_json: Value =
        serde_json::from_slice(&to_bytes(overview.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(overview_json.get("data").is_some());
    assert!(overview_json.get("meta").is_some());

    let search = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?q=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);

    let frag = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(frag.status(), StatusCode::OK);
    let frag_body = String::from_utf8(
        to_bytes(frag.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(frag_body.contains("overview-chart"));
}

fn seed_workspace(workspace: &std::path::Path) {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(workspace, &config).unwrap();

    let store = SqliteStore::open(workspace).unwrap();
    store
        .upsert_symbol(SymbolRecord {
            id: "sym-test".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "test::symbol".to_owned(),
            signature_fingerprint: "sig".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .unwrap();
}
