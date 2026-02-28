use std::sync::Arc;

use aether_config::{AetherConfig, GraphBackend, save_workspace_config};
use aether_core::{EdgeKind, SymbolEdge};
use aether_store::{DriftAnalysisStateRecord, SirMetaRecord, SqliteStore, Store, SymbolRecord};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::{SharedState, dashboard_router};

#[tokio::test]
async fn overview_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json["data"].get("total_symbols").is_some());
    assert!(json["data"].get("total_files").is_some());
    assert!(json["data"].get("sir_coverage_pct").is_some());
    assert!(json["data"].get("languages").is_some());
    assert!(json["meta"].get("generated_at").is_some());
}

#[tokio::test]
async fn search_api_returns_results() {
    let (_tmp, app, ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?q=demo&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let results = json["data"]["results"].as_array().unwrap();
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r["symbol_id"] == ids.primary));
    assert!(results.iter().all(|r| r.get("sir_exists").is_some()));
}

#[tokio::test]
async fn overview_fragment_contains_stat_cards_and_chart() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("stat-card"));
    assert!(body.contains("id=\"overview-chart\""));
    assert!(body.contains("data-table"));
}

#[tokio::test]
async fn search_fragment_contains_clickable_results() {
    let (_tmp, app, ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/search?q=demo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("class=\"clickable\""));
    assert!(body.contains(&format!("/dashboard/frag/symbol/{}", ids.primary)));
    assert!(body.contains("hx-target=\"#detail-panel\""));
}

#[tokio::test]
async fn symbol_detail_fragment_renders_sir_blocks() {
    let (_tmp, app, ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/dashboard/frag/symbol/{}", ids.primary))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("sir-block"));
    assert!(body.contains("Run demo task"));
    assert!(body.contains("closeDetailPanel()"));
}

#[tokio::test]
async fn static_shell_serves_index_with_htmx() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("htmx.min.js"));
    assert!(body.contains("/dashboard/static/style.css"));
}

#[tokio::test]
async fn graph_api_returns_nodes_array_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/graph?limit=5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json["data"]["nodes"].is_array());
    assert!(json["data"]["edges"].is_array());
}

#[tokio::test]
async fn drift_api_returns_entries_array_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drift")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json["data"]["drift_entries"].is_array());
}

#[tokio::test]
async fn coupling_api_returns_pairs_array_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/coupling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json["data"]["pairs"].is_array());
}

#[tokio::test]
async fn health_api_returns_dimensions_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json.get("meta").is_some());
    assert!(json["data"]["dimensions"].is_object());
}

#[tokio::test]
async fn graph_fragment_contains_chart_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/graph")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("id=\"graph-container\""));
}

#[tokio::test]
async fn drift_fragment_contains_chart_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/drift-table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("id=\"drift-chart\""));
}

#[tokio::test]
async fn coupling_fragment_contains_chart_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/coupling")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("id=\"heatmap-container\""));
}

#[tokio::test]
async fn health_fragment_contains_chart_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("id=\"health-chart\""));
}

#[tokio::test]
async fn unknown_static_path_returns_404() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/static/does-not-exist.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

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

fn seed_workspace(workspace: &std::path::Path) {
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(workspace, &config).unwrap();

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

    store
        .upsert_sir_meta(SirMetaRecord {
            id: "sym-demo-run".to_owned(),
            sir_hash: "hash-demo-run".to_owned(),
            sir_version: 1,
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
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
}
