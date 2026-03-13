use std::fs;
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
async fn anatomy_api_returns_expected_sections() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/anatomy")
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
    assert!(json["data"].get("project_name").is_some());
    assert!(json["data"].get("summary").is_some());
    assert!(json["data"]["maturity"].is_object());
    assert!(json["data"]["tech_stack"].is_array());
    assert!(json["data"]["layers"].is_array());
    assert!(json["data"]["key_actors"].is_array());
    assert!(json["data"]["simplified_graph"]["nodes"].is_array());
    assert!(json["data"]["simplified_graph"]["edges"].is_array());
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
    let first = results.first().unwrap();
    assert!(first.get("sir_summary").is_some());
    assert!(first.get("risk_score").is_some());
    assert!(first.get("pagerank").is_some());
    assert!(first.get("drift_score").is_some());
    assert!(first.get("test_count").is_some());
    assert!(first.get("related_symbols").is_some());
}

#[tokio::test]
async fn changes_api_returns_shape() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/changes?since=24h&limit=20")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json["data"]["period"].is_string());
    assert!(json["data"]["change_count"].is_number());
    assert!(json["data"]["changes"].is_array());
    assert!(json["data"]["file_summary"].is_object());
}

#[tokio::test]
async fn ask_api_returns_envelope_and_summary() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"question":"demo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json["data"]["question"].is_string());
    assert!(json["data"]["answer_type"].is_string());
    assert!(json["data"]["results"].is_array());
    assert!(json["data"]["summary"].is_string());
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
async fn overview_fragment_contains_recent_changes_loader() {
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
    assert!(body.contains("id=\"overview-recent-changes\""));
    assert!(body.contains("/dashboard/frag/changes?since=24h&amp;limit=20&amp;embed=true"));
}

#[tokio::test]
async fn anatomy_fragment_contains_layer_graph_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/anatomy")
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
    assert!(body.contains("id=\"anatomy-layer-graph\""));
    assert!(body.contains("Project Layers"));
}

#[tokio::test]
async fn anatomy_layer_fragment_contains_file_summaries() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/anatomy/layer?name=Core%20Logic")
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
    assert!(body.contains("Core Logic"));
    assert!(body.contains("Show symbols"));
}

#[tokio::test]
async fn anatomy_file_fragment_contains_symbol_links_and_sir() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/anatomy/file?path=src/lib.rs")
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
    assert!(body.contains("symbol-link text-blue-600 hover:underline cursor-pointer"));
    assert!(body.contains("SIR Intent"));
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
    assert!(body.contains("data-page=\"search\""));
    assert!(body.contains("id=\"smart-search-results\""));
    assert!(body.contains("Risk: loading"));
    assert!(body.contains(&format!(
        "/dashboard/frag/blast-radius?symbol_id={}",
        ids.primary
    )));
}

#[tokio::test]
async fn changes_fragment_renders_content() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/changes?since=24h&limit=20")
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
    assert!(body.contains("What Changed Recently"));
    assert!(body.contains("id=\"changes-content\""));
}

#[tokio::test]
async fn ask_fragment_renders_related_components() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/dashboard/frag/ask")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("question=demo"))
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
    assert!(body.contains("Related Components"));
    assert!(body.contains("symbol-link"));
}

#[tokio::test]
async fn ask_fragment_shows_index_message_when_unavailable() {
    let (_tmp, app) = empty_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/dashboard/frag/ask")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("question=demo"))
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
    assert!(body.contains("AETHER needs to index this project before it can answer questions"));
}

#[tokio::test]
async fn symbol_fragment_renders_narrative_sections() {
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
    assert!(body.contains("Symbol Deep Dive"));
    assert!(body.contains("How It Fits"));
    assert!(body.contains("How It Gets Used"));
    assert!(body.contains("Side Effects &amp; Risks"));
    assert!(body.contains("Run demo task"));
}

#[tokio::test]
async fn tour_api_returns_stops_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/tour")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json["data"]["stop_count"].is_number());
    assert!(json["data"]["stops"].is_array());
}

#[tokio::test]
async fn glossary_api_returns_terms_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/glossary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json["data"]["terms"].is_array());
    assert!(json["data"]["total"].is_number());
}

#[tokio::test]
async fn file_api_returns_file_narrative() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/file/src%2Flib.rs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["path"].is_string());
    assert!(json["data"]["summary"].is_string());
    assert!(json["data"]["symbols"].is_array());
}

#[tokio::test]
async fn flow_api_returns_steps_for_start_symbol() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/flow?start=main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["steps"].is_array());
    assert!(json["data"]["step_count"].is_number());
}

#[tokio::test]
async fn flow_api_returns_not_found_for_disconnected_path() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/flow?start=helper&end=main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn flow_fragment_renders_builder_and_timeline() {
    let (_tmp, app, _ids) = seeded_app().await;
    let builder = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/flow")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(builder.status(), StatusCode::OK);
    let builder_body = String::from_utf8(
        to_bytes(builder.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(builder_body.contains("Trace Flow"));
    assert!(builder_body.contains("Try tracing from"));

    let timeline = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/flow?start=main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(timeline.status(), StatusCode::OK);
    let timeline_body = String::from_utf8(
        to_bytes(timeline.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(timeline_body.contains("Step 1"));
    assert!(timeline_body.contains("symbol-link text-blue-600 hover:underline cursor-pointer"));
}

#[tokio::test]
async fn glossary_and_tour_fragments_render() {
    let (_tmp, app, _ids) = seeded_app().await;
    let glossary = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/glossary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(glossary.status(), StatusCode::OK);
    let glossary_body = String::from_utf8(
        to_bytes(glossary.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(glossary_body.contains("📚 Glossary"));
    assert!(glossary_body.contains("Spec"));

    let tour = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/tour")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tour.status(), StatusCode::OK);
    let tour_body = String::from_utf8(
        to_bytes(tour.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(tour_body.contains("🗺️ Guided Tour"));
    assert!(tour_body.contains("tour-content"));
}

#[tokio::test]
async fn file_fragment_renders_file_narrative_page() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/file/src%2Flib.rs")
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
    assert!(body.contains("File Deep Dive"));
    assert!(body.contains("How This File Works"));
    assert!(body.contains("All Components In This File"));
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
    assert!(body.contains("localStorage.theme"));
    assert!(body.contains("id=\"theme-toggle\""));
    assert!(body.contains("hx-get=\"/dashboard/frag/anatomy\""));
    assert!(body.contains("id=\"ask-container\""));
    assert!(body.contains("hx-post=\"/dashboard/frag/ask\""));
    assert!(body.contains("🕐 Recent Changes"));
    assert!(body.contains("hx-get=\"/dashboard/frag/changes\""));
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
async fn dashboard_health_score_endpoint() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health-score?limit=5&max_score=100")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(body.as_ref())
    );
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("data").is_some());
    assert!(json["data"]["workspace_score"].is_number());
    assert!(json["data"]["severity"].is_string());
    assert!(json["data"]["delta"].is_number());
    assert!(json["data"]["crates"].is_array());
    assert!(json["data"]["archetype_distribution"].is_object());
    assert!(json["data"]["trend"].is_array());
}

#[tokio::test]
async fn health_score_fragment_contains_hotspot_table_and_sparkline_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/health-score?limit=5&max_score=100")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Workspace Health Score"));
    assert!(body.contains("Hotspot Crates"));
    assert!(body.contains("data-health-score-trend"));
}

#[tokio::test]
async fn overview_fragment_contains_health_score_loader_below_llm_difficulty() {
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
    let difficulty_index = body.find("LLM Difficulty Analysis").unwrap();
    let panel_index = body.find("id=\"overview-health-score-panel\"").unwrap();
    assert!(panel_index > difficulty_index);
    assert!(body.contains("hx-get=\"/dashboard/frag/health-score\""));
}

#[tokio::test]
async fn xray_api_returns_metrics_hotspots_and_envelope() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/xray?window=7d")
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
    assert!(json["data"]["metrics"].is_object());
    assert!(json["data"]["hotspots"].is_array());
}

#[tokio::test]
async fn xray_api_empty_data_returns_not_computed_null_metrics() {
    let (_tmp, app) = empty_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/xray?window=7d")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let metrics = &json["data"]["metrics"];
    for metric in [
        "sir_coverage",
        "orphan_count",
        "avg_drift",
        "graph_connectivity",
        "high_coupling_pairs",
        "sir_coverage_pct",
        "index_freshness_secs",
        "risk_grade",
    ] {
        assert!(metrics[metric]["value"].is_null());
        assert_eq!(metrics[metric]["not_computed"].as_bool(), Some(true));
    }
}

#[tokio::test]
async fn blast_radius_invalid_symbol_returns_404() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blast-radius?symbol_id=does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn architecture_api_empty_returns_not_computed() {
    let (_tmp, app) = empty_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/architecture?granularity=symbol")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["not_computed"].as_bool(), Some(true));
    assert!(json["data"]["communities"].as_array().is_some());
    assert!(json["data"]["symbols"].as_array().is_some());
}

#[tokio::test]
async fn time_machine_api_returns_snapshot_shape() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/time-machine?at=2026-01-01T00:00:00Z&layers=deps,drift")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["nodes"].is_array());
    assert!(json["data"]["edges"].is_array());
    assert!(json["data"]["events"].is_array());
    assert!(json["data"]["time_range"].is_object());
}

#[tokio::test]
async fn causal_chain_api_returns_shape_and_envelope() {
    let (_tmp, app, ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/causal-chain?symbol_id={}&depth=3&lookback=30d",
                    ids.primary
                ))
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
    assert!(json["data"]["target"].is_object());
    assert!(json["data"]["chain"].is_array());
    assert!(json["data"]["overall_confidence"].is_number());
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
async fn xray_fragment_contains_metric_grid() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/xray")
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
    assert!(body.contains("id=\"xray-metrics-grid\""));
    assert!(body.contains("id=\"xray-hotspots-body\""));
}

#[tokio::test]
async fn blast_radius_fragment_contains_search_and_controls() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/blast-radius")
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
    assert!(body.contains("id=\"blast-symbol-input\""));
    assert!(body.contains("id=\"blast-depth\""));
    assert!(body.contains("id=\"blast-min-coupling\""));
    assert!(body.contains("id=\"blast-radius-chart\""));
}

#[tokio::test]
async fn architecture_fragment_contains_treemap_container() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/architecture")
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
    assert!(body.contains("id=\"architecture-treemap\""));
}

#[tokio::test]
async fn time_machine_fragment_contains_timeline_controls() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/time-machine")
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
    assert!(body.contains("id=\"time-machine-at\""));
    assert!(body.contains("id=\"time-machine-graph\""));
    assert!(body.contains("id=\"time-machine-events\""));
}

#[tokio::test]
async fn causal_fragment_contains_search_and_animate_button() {
    let (_tmp, app, _ids) = seeded_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/causal")
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
    assert!(body.contains("id=\"causal-symbol-input\""));
    assert!(body.contains("id=\"causal-animate\""));
    assert!(body.contains("id=\"causal-graph\""));
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

    store
        .upsert_sir_meta(SirMetaRecord {
            id: "sym-demo-run".to_owned(),
            sir_hash: "hash-demo-run".to_owned(),
            sir_version: 1,
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            generation_pass: "single".to_owned(),
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
