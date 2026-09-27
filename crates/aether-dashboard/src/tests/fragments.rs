//! HTMX fragment tests: each page fragment renders its containers and controls.

use super::*;

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
