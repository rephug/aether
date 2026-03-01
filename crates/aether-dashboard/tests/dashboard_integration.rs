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
async fn dashboard_router_serves_stage79_routes_and_preserves_existing_endpoints() {
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
    let shell_body = String::from_utf8(
        to_bytes(shell.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(shell_body.contains("localStorage.theme"));
    assert!(shell_body.contains("id=\"theme-toggle\""));

    let xray = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/xray?window=7d")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(xray.status(), StatusCode::OK);
    let xray_json: Value =
        serde_json::from_slice(&to_bytes(xray.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(xray_json["data"]["metrics"].is_object());
    assert!(xray_json["data"]["hotspots"].is_array());

    let blast = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/blast-radius?symbol_id=nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blast.status(), StatusCode::NOT_FOUND);

    let architecture = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/architecture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(architecture.status(), StatusCode::OK);
    let architecture_json: Value = serde_json::from_slice(
        &to_bytes(architecture.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(architecture_json["data"].is_object());

    let time_machine = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/time-machine?at=2026-01-01T00:00:00Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(time_machine.status(), StatusCode::OK);
    let time_json: Value = serde_json::from_slice(
        &to_bytes(time_machine.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(time_json["data"]["nodes"].is_array());
    assert!(time_json["data"]["edges"].is_array());
    assert!(time_json["data"]["events"].is_array());

    let causal = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/causal-chain?symbol_id=test&depth=3&lookback=30d")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(causal.status(), StatusCode::OK);
    let causal_json: Value =
        serde_json::from_slice(&to_bytes(causal.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(causal_json["data"]["target"].is_object());
    assert!(causal_json["data"]["chain"].is_array());

    let xray_fragment = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/xray")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(xray_fragment.status(), StatusCode::OK);
    let xray_fragment_body = String::from_utf8(
        to_bytes(xray_fragment.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(xray_fragment_body.contains("xray-metrics-grid"));

    // Regression check for original endpoints from stage 7.6.
    for endpoint in [
        "/api/v1/overview",
        "/api/v1/search?q=test",
        "/api/v1/graph?limit=5",
        "/api/v1/drift",
        "/api/v1/coupling",
        "/api/v1/health",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(endpoint)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "endpoint {}", endpoint);
        let json: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert!(json.get("data").is_some(), "endpoint {}", endpoint);
        assert!(json.get("meta").is_some(), "endpoint {}", endpoint);
    }
}

fn seed_workspace(workspace: &std::path::Path) {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(workspace, &config).unwrap();

    let store = SqliteStore::open(workspace).unwrap();
    store
        .upsert_symbol(SymbolRecord {
            id: "test".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "test::symbol".to_owned(),
            signature_fingerprint: "sig".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .unwrap();
}
