//! JSON API endpoint tests: response shapes and envelopes.

use super::*;

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
async fn search_api_applies_filters_before_limit() {
    let (_tmp, app) = seeded_app_with_python_symbol().await;

    // The only Python match sorts after three Rust matches, so it is outside
    // a store window of `limit` rows.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?q=demo&lang=python&limit=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let ids = json["data"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["symbol_id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![PYTHON_SYMBOL_ID.to_owned()]);
    assert_eq!(json["data"]["count"], 1);
}

#[tokio::test]
async fn search_api_filtered_results_still_respect_limit() {
    let (_tmp, app) = seeded_app_with_python_symbol().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?q=demo&lang=rust&limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let results = json["data"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|row| row["language"] == "rust"));
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

fn symbol_access_state(workspace: &std::path::Path, symbol_id: &str) -> (i64, Option<i64>) {
    let conn = Connection::open_with_flags(
        workspace.join(".aether/meta.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1",
        params![symbol_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

#[tokio::test]
async fn ask_api_does_not_record_symbol_access() {
    let (tmp, app, ids) = seeded_app().await;
    let before = symbol_access_state(tmp.path(), ids.primary.as_str());

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"question":"run"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let results = json["data"]["results"].as_array().unwrap();
    assert!(
        results.iter().any(|r| r["id"] == ids.primary),
        "ask should return the seeded symbol: {json}"
    );

    // The dashboard is read-only: answering a question must not bump the
    // access counters that feed recall ranking.
    let after = symbol_access_state(tmp.path(), ids.primary.as_str());
    assert_eq!(after, before);
}

fn sir_json_column(workspace: &std::path::Path, symbol_id: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        workspace.join(".aether/meta.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    conn.query_row(
        "SELECT sir_json FROM sir WHERE id = ?1",
        params![symbol_id],
        |row| row.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn ask_api_answers_from_a_legacy_sir_mirror_on_the_read_only_store() {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    // A symbol from the file-mirror era: its SIR row holds no JSON and the
    // blob lives only in .aether/sir/<id>.json.
    let legacy_id = "sym-demo-helper";
    {
        let store = SqliteStore::open(temp.path()).unwrap();
        store
            .upsert_sir_meta(SirMetaRecord {
                id: legacy_id.to_owned(),
                sir_hash: "hash-demo-helper".to_owned(),
                sir_version: 1,
                provider: "mock".to_owned(),
                model: "mock-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_100,
                sir_status: "ready".to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_100,
            })
            .unwrap();
    }
    fs::write(
        temp.path().join(format!(".aether/sir/{legacy_id}.json")),
        r#"{"intent":"Help from the mirror","dependencies":["ctx"]}"#,
    )
    .unwrap();
    assert_eq!(sir_json_column(temp.path(), legacy_id), None);

    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/ask")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"question":"helper"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "ask failed: {json}");

    let results = json["data"]["results"].as_array().unwrap();
    let legacy = results
        .iter()
        .find(|r| r["id"] == legacy_id)
        .unwrap_or_else(|| panic!("ask should return the legacy symbol: {json}"));
    assert_eq!(legacy["snippet"], "Help from the mirror");
    assert_eq!(legacy["sir_intent"], "Help from the mirror");

    // The read-only dashboard serves the mirror without backfilling the row.
    assert_eq!(sir_json_column(temp.path(), legacy_id), None);
}

#[tokio::test]
async fn enhance_api_rejects_empty_prompt() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/enhance")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"prompt":"   ","budget":8000}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "bad_request");
    assert_eq!(json["message"], "prompt must not be empty");
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
    assert!(body.contains("Recent Changes"));
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

async fn blast_radius_rings(app: axum::Router, symbol_id: &str) -> (Vec<Vec<String>>, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/blast-radius?symbol_id={symbol_id}&depth=2&min_coupling=0.0"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].clone();
    let rings = data["rings"]
        .as_array()
        .expect("rings array")
        .iter()
        .map(|ring| {
            ring["nodes"]
                .as_array()
                .expect("ring nodes array")
                .iter()
                .map(|node| node["symbol_id"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    (rings, data)
}

#[tokio::test]
async fn blast_radius_walks_callers_not_callees() {
    // Seed call graph: main -> run -> helper. Changing a symbol affects the
    // symbols that call it, so the rings must walk callers outward.
    let (_tmp, app, _ids) = seeded_app().await;

    let (helper_rings, helper_data) = blast_radius_rings(app.clone(), "sym-demo-helper").await;
    assert_eq!(
        helper_rings,
        vec![
            vec!["sym-demo-run".to_owned()],
            vec!["sym-demo-main".to_owned()],
        ]
    );
    assert_eq!(helper_data["total_impacted"].as_u64(), Some(2));
    assert_eq!(
        helper_data["rings"][1]["nodes"][0]["parent_symbol_id"].as_str(),
        Some("sym-demo-run")
    );

    let (run_rings, run_data) = blast_radius_rings(app, "sym-demo-run").await;
    assert_eq!(
        run_rings,
        vec![vec!["sym-demo-main".to_owned()], Vec::<String>::new()]
    );
    assert_eq!(run_data["total_impacted"].as_u64(), Some(1));
}

#[tokio::test]
async fn blast_radius_follows_call_edges_only() {
    // A type reference is not a call, so it must not pull the referencing
    // symbol into the blast radius of the referenced one.
    let (tmp, app, _ids) = seeded_app().await;
    SqliteStore::open(tmp.path())
        .unwrap()
        .upsert_edges(&[SymbolEdge {
            source_id: "sym-demo-helper".to_owned(),
            target_qualified_name: "demo::main".to_owned(),
            edge_kind: EdgeKind::TypeRef,
            file_path: "src/lib.rs".to_owned(),
        }])
        .unwrap();

    let (main_rings, main_data) = blast_radius_rings(app.clone(), "sym-demo-main").await;
    assert_eq!(main_rings, vec![Vec::<String>::new(), Vec::<String>::new()]);
    assert_eq!(main_data["total_impacted"].as_u64(), Some(0));

    let (helper_rings, _) = blast_radius_rings(app, "sym-demo-helper").await;
    assert_eq!(
        helper_rings,
        vec![
            vec!["sym-demo-run".to_owned()],
            vec!["sym-demo-main".to_owned()],
        ]
    );
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

/// The causal explorer's lookback select sends `7d`, `30d` and `90d`. Each must be a
/// window of that many days, not of that many commits. The workspace has no git
/// repository and the SIR history rows carry no commit hash, so only a day window can
/// include them. Timestamps are in seconds, as the indexer records them.
#[tokio::test]
async fn causal_chain_api_lookback_select_values_are_day_windows() {
    use aether_store::{
        GraphStore, ResolvedEdge, SirHistoryStore, SurrealGraphStore, block_on_store_future,
    };

    const DAY_SECS: i64 = 24 * 60 * 60;

    let temp = TempDir::new().unwrap();
    let workspace = temp.path().to_path_buf();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Surreal;
    config.embeddings.enabled = false;
    save_workspace_config(&workspace, &config).unwrap();

    let symbol = |id: &str, qualified_name: &str, file_path: &str| SymbolRecord {
        id: id.to_owned(),
        file_path: file_path.to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: 1_700_000_000,
    };
    let target = symbol("sym-causal-target", "demo::target", "src/lib.rs");
    let recent = symbol("sym-causal-recent", "demo::recent", "src/recent.rs");
    let older = symbol("sym-causal-older", "demo::older", "src/older.rs");

    let store = SqliteStore::open(&workspace).unwrap();
    for record in [&target, &recent, &older] {
        store.upsert_symbol(record.clone()).unwrap();
    }
    let now_secs = chrono::Utc::now().timestamp();
    // `recent` last changed 2 days ago and `older` 45 days ago.
    let changed_days_ago_by_symbol = [(recent.id.as_str(), 2_i64), (older.id.as_str(), 45)];
    for (symbol_id, changed_days_ago) in changed_days_ago_by_symbol {
        store
            .record_sir_version_if_changed(
                symbol_id,
                "hash-v1",
                "mock",
                "mock",
                r#"{"purpose":"old purpose","edge_cases":[],"dependencies":[]}"#,
                now_secs - (changed_days_ago + 1) * DAY_SECS,
                None,
            )
            .unwrap();
        store
            .record_sir_version_if_changed(
                symbol_id,
                "hash-v2",
                "mock",
                "mock",
                r#"{"purpose":"new purpose","edge_cases":["timeout"],"dependencies":["queue"]}"#,
                now_secs - changed_days_ago * DAY_SECS,
                None,
            )
            .unwrap();
    }
    drop(store);

    let graph_records = [target.clone(), recent.clone(), older.clone()];
    block_on_store_future(async move {
        // The first SurrealKV open has a 5 s timeout, which a heavily loaded test machine
        // can hit. Later opens reuse the cached handle, so only this one needs a retry.
        let mut attempts_left = 3;
        let graph = loop {
            attempts_left -= 1;
            match SurrealGraphStore::open(&workspace).await {
                Ok(graph) => break graph,
                Err(err) if attempts_left == 0 => panic!("open Surreal graph store: {err}"),
                Err(_) => continue,
            }
        };
        for record in &graph_records {
            graph.upsert_symbol_node(record).await.unwrap();
        }
        for upstream in &graph_records[1..] {
            graph
                .upsert_edge(&ResolvedEdge {
                    source_id: graph_records[0].id.clone(),
                    target_id: upstream.id.clone(),
                    edge_kind: EdgeKind::Calls,
                    file_path: graph_records[0].file_path.clone(),
                })
                .await
                .unwrap();
        }
    })
    .unwrap();

    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);

    for (lookback, expected) in [
        ("7d", vec![recent.id.as_str()]),
        ("30d", vec![recent.id.as_str()]),
        ("90d", vec![older.id.as_str(), recent.id.as_str()]),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/causal-chain?symbol_id={}&depth=3&lookback={lookback}",
                        target.id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let chain = json["data"]["chain"].as_array().unwrap();
        let mut chain_ids = chain
            .iter()
            .map(|row| row["symbol_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        chain_ids.sort();
        assert_eq!(
            chain_ids, expected,
            "lookback={lookback} should cover the last {lookback} of SIR changes; response: {json}"
        );

        // Each row's timestamp is when that symbol's SIR changed, not the request time.
        // Without commit metadata the analyzer reports the change date as an epoch
        // number in seconds, which the dashboard must convert to milliseconds.
        for row in chain {
            let symbol_id = row["symbol_id"].as_str().unwrap();
            let changed_days_ago = changed_days_ago_by_symbol
                .iter()
                .find(|(id, _)| *id == symbol_id)
                .map(|(_, days)| *days)
                .unwrap();
            assert_eq!(
                row["timestamp"].as_i64(),
                Some((now_secs - changed_days_ago * DAY_SECS) * 1000),
                "lookback={lookback}: {symbol_id} timestamp should be its change time; response: {json}"
            );
        }
    }
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

#[tokio::test]
async fn contracts_api_returns_ok() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/contracts")
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
    assert!(json["data"].get("contracts").is_some());
    assert!(json["data"]["contracts"].is_array());
    assert!(json["data"].get("summary").is_some());
    assert!(json["data"]["summary"].get("total_contracts").is_some());
    assert!(json["data"]["summary"].get("satisfaction_rate").is_some());
    assert!(json["data"].get("recent_violations").is_some());
    assert!(json["data"]["recent_violations"].is_array());
}

const CONTEXT_BUILD_OUTSIDE_MARKER: &str = "CONTEXT_BUILD_OUTSIDE_MARKER";

async fn post_context_build(app: axum::Router, body: Value) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/context/build")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn context_build_content(body: &str) -> String {
    let json: Value = serde_json::from_str(body).unwrap();
    json["data"]["content"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn context_build_rejects_targets_outside_workspace() {
    let (_tmp, app, _ids) = seeded_app().await;
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("outside_secret.txt");
    fs::write(&secret, format!("{CONTEXT_BUILD_OUTSIDE_MARKER}\n")).unwrap();
    let outside_name = outside.path().file_name().unwrap().to_str().unwrap();

    for target in [
        secret.to_string_lossy().into_owned(),
        format!("../{outside_name}/outside_secret.txt"),
    ] {
        let (status, body) = post_context_build(
            app.clone(),
            serde_json::json!({ "targets": [target.as_str()] }),
        )
        .await;
        assert!(
            !body.contains(CONTEXT_BUILD_OUTSIDE_MARKER),
            "target {target} leaked a file outside the workspace: {body}"
        );
        assert_eq!(status, StatusCode::BAD_REQUEST, "target {target}: {body}");
        let json: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["error"], "bad_request");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn context_build_does_not_follow_symlinks_out_of_workspace() {
    let (tmp, app, _ids) = seeded_app().await;
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("outside_secret.txt");
    fs::write(&secret, format!("{CONTEXT_BUILD_OUTSIDE_MARKER}\n")).unwrap();
    std::os::unix::fs::symlink(&secret, tmp.path().join("src/escape.txt")).unwrap();

    let (status, body) =
        post_context_build(app, serde_json::json!({ "targets": ["src/escape.txt"] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains(CONTEXT_BUILD_OUTSIDE_MARKER),
        "symlinked target leaked a file outside the workspace: {body}"
    );
}

#[tokio::test]
async fn context_build_truncates_multibyte_source_on_char_boundary() {
    let (tmp, app, _ids) = seeded_app().await;
    // One ASCII byte followed by 3-byte characters puts char boundaries at 1 + 3k.
    // Budget 100 gives the source layer 30 tokens, a 105-byte cut that lands
    // inside a character.
    fs::write(
        tmp.path().join("src/wide.txt"),
        format!("a{}", "─".repeat(200)),
    )
    .unwrap();

    let (status, body) = post_context_build(
        app,
        serde_json::json!({ "targets": ["src/wide.txt"], "budget": 100 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let content = context_build_content(&body);
    assert!(content.contains("### Source"), "{content}");
    assert!(content.contains("... (truncated)"), "{content}");
}

#[tokio::test]
async fn context_build_summarizes_multibyte_neighbor_intent_on_char_boundary() {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    // The caller of demo::helper gets an intent whose byte 77 sits inside an em dash.
    let intent = format!("{}— and more text past the summary limit", "a".repeat(76));
    let store = SqliteStore::open(temp.path()).unwrap();
    store
        .write_sir_blob(
            "sym-demo-run",
            &serde_json::json!({ "intent": intent }).to_string(),
        )
        .unwrap();
    drop(store);
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let app = dashboard_router(state);

    let (status, body) =
        post_context_build(app, serde_json::json!({ "targets": ["src/lib.rs"] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let content = context_build_content(&body);
    assert!(content.contains("### Dependencies & Callers"), "{content}");
    assert!(
        content.contains(&format!("{}—...", "a".repeat(76))),
        "{content}"
    );
}

#[tokio::test]
async fn context_build_clamps_oversized_budget() {
    let (_tmp, app, _ids) = seeded_app().await;

    let (status, body) = post_context_build(
        app,
        serde_json::json!({ "targets": ["src/lib.rs"], "budget": u64::MAX }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["data"]["budget_usage"]["total"].as_u64(),
        Some(1_000_000)
    );
    assert!(context_build_content(&body).contains("### Source"));
}

const INFERENCE_FORM: &str = "provider=openai_compat&endpoint=https%3A%2F%2Fattacker.example%2Fv1&model=x&api_key_env=GEMINI_API_KEY&concurrency=4";

fn settings_post(uri: &str, headers: &[(&str, &str)], body: &'static str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body)).unwrap()
}

#[tokio::test]
async fn cross_origin_writes_are_rejected_and_leave_config_unchanged() {
    let (tmp, app) = empty_app().await;
    let config_path = aether_config::config_path(tmp.path());
    let before = fs::read_to_string(&config_path).unwrap();

    let cases: &[(&str, &[(&str, &str)])] = &[
        // Cross-site form post from an arbitrary web page.
        (
            "/api/v1/settings/inference",
            &[
                ("host", "127.0.0.1:9730"),
                ("origin", "https://evil.example"),
                ("sec-fetch-site", "cross-site"),
            ],
        ),
        // Same check without fetch metadata (older browsers).
        (
            "/api/v1/settings/inference",
            &[
                ("host", "127.0.0.1:9730"),
                ("origin", "https://evil.example"),
            ],
        ),
        // Fetch metadata alone is enough to reject.
        (
            "/api/v1/settings/inference",
            &[("host", "127.0.0.1:9730"), ("sec-fetch-site", "cross-site")],
        ),
        // Another local server on a different port is a different origin.
        // Without fetch metadata (older browsers) only the Origin/Host port
        // comparison rejects these.
        (
            "/api/v1/settings/inference",
            &[
                ("host", "127.0.0.1:9730"),
                ("origin", "http://127.0.0.1:3000"),
            ],
        ),
        (
            "/api/v1/settings/inference",
            &[
                ("host", "localhost:9730"),
                ("origin", "http://localhost:3000"),
            ],
        ),
        // The same request from a browser that sends fetch metadata, which
        // labels another port on the same host as same-site.
        (
            "/api/v1/settings/inference",
            &[
                ("host", "localhost:9730"),
                ("origin", "http://localhost:3000"),
                ("sec-fetch-site", "same-site"),
            ],
        ),
        // Opaque origin (sandboxed iframe, file:// page).
        (
            "/api/v1/settings/inference",
            &[("host", "127.0.0.1:9730"), ("origin", "null")],
        ),
        // DNS rebinding: Origin matches Host but is not a loopback name.
        (
            "/api/v1/settings/inference",
            &[
                ("host", "evil.example:9730"),
                ("origin", "http://evil.example:9730"),
                ("sec-fetch-site", "same-origin"),
            ],
        ),
        // Body-less reset endpoint.
        (
            "/api/v1/settings/inference/reset",
            &[
                ("host", "127.0.0.1:9730"),
                ("origin", "https://evil.example"),
            ],
        ),
        // Ask fragment is form-based too.
        (
            "/dashboard/frag/ask",
            &[
                ("host", "127.0.0.1:9730"),
                ("origin", "https://evil.example"),
            ],
        ),
    ];

    for (uri, headers) in cases {
        let body = if uri.ends_with("/ask") {
            "question=demo"
        } else {
            INFERENCE_FORM
        };
        let response = app
            .clone()
            .oneshot(settings_post(uri, headers, body))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "expected 403 for POST {uri} with {headers:?}"
        );
    }

    assert_eq!(fs::read_to_string(&config_path).unwrap(), before);
}

#[tokio::test]
async fn same_origin_and_non_browser_writes_are_allowed() {
    let (tmp, app) = empty_app().await;
    let config_path = aether_config::config_path(tmp.path());

    let cases: &[&[(&str, &str)]] = &[
        // htmx request from the dashboard page served on 127.0.0.1.
        &[
            ("host", "127.0.0.1:9730"),
            ("origin", "http://127.0.0.1:9730"),
            ("sec-fetch-site", "same-origin"),
        ],
        // Same page opened as localhost, on any port the server is bound to.
        &[
            ("host", "localhost:41234"),
            ("origin", "http://localhost:41234"),
        ],
        // curl, the VS Code extension and other non-browser clients.
        &[],
    ];

    for headers in cases {
        let response = app
            .clone()
            .oneshot(settings_post(
                "/api/v1/settings/inference",
                headers,
                INFERENCE_FORM,
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "expected 200 for {headers:?}"
        );
    }
    assert!(
        fs::read_to_string(&config_path)
            .unwrap()
            .contains("attacker.example")
    );

    // Reads stay open to any origin; the browser keeps cross-origin pages
    // from reading the response.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/settings/inference")
                .header("host", "127.0.0.1:9730")
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn post_settings_form(app: &axum::Router, section: &str, form: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/settings/{section}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn get_settings_json(app: &axum::Router, section: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/settings/{section}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn settings_save_tiered_without_tiered_fields_keeps_tiered_defaults() {
    let (_tmp, app) = empty_app().await;

    // What the inference form submitted after switching the provider to Tiered
    // while [inference.tiered] was unset: no tiered.* inputs at all.
    let (status, body) = post_settings_form(
        &app,
        "inference",
        "provider=tiered&model=&api_key_env=GEMINI_API_KEY&concurrency=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, json) = get_settings_json(&app, "inference").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["provider"], "tiered");
    let tiered = &json["data"]["tiered"];
    assert_eq!(tiered["primary_threshold"].as_f64(), Some(0.8), "{tiered}");
    assert_eq!(tiered["retry_with_fallback"], Value::Bool(true), "{tiered}");
}

#[tokio::test]
async fn settings_save_tiered_without_tiered_fields_keeps_existing_tiered_table() {
    let (tmp, app) = empty_app().await;

    let stored_tiered = aether_config::TieredConfig {
        primary: "openai_compat".to_owned(),
        primary_model: Some("gpt-4o-mini".to_owned()),
        primary_endpoint: Some("https://primary.example/v1".to_owned()),
        primary_api_key_env: "PRIMARY_KEY".to_owned(),
        primary_threshold: 0.6,
        fallback_model: Some("fallback-model".to_owned()),
        fallback_endpoint: Some("https://fallback.example/v1".to_owned()),
        retry_with_fallback: false,
    };
    let mut config = aether_config::load_workspace_config(tmp.path()).unwrap();
    config.inference.provider = aether_config::InferenceProviderKind::Tiered;
    config.inference.tiered = Some(stored_tiered.clone());
    save_workspace_config(tmp.path(), &config).unwrap();

    // A hand-made save that only changes concurrency and sends no tiered.* fields
    // must leave the stored tiered table as it is.
    let (status, body) = post_settings_form(
        &app,
        "inference",
        "provider=tiered&api_key_env=GEMINI_API_KEY&concurrency=8",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, json) = get_settings_json(&app, "inference").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["provider"], "tiered");
    assert_eq!(json["data"]["concurrency"], 8);
    assert_eq!(
        json["data"]["tiered"],
        serde_json::to_value(&stored_tiered).unwrap()
    );
}

#[tokio::test]
async fn settings_save_rejects_values_that_would_make_config_unloadable() {
    let (tmp, app) = empty_app().await;
    let config_path = aether_config::config_path(tmp.path());
    let before = fs::read(&config_path).unwrap();

    for (section, form) in [
        ("inference", "provider=bogus&api_key_env=X&concurrency=4"),
        ("inference", "api_key_env=X&concurrency=4"),
        (
            "search",
            "reranker=bogus&rerank_window=50&thresholds.default=0.5&thresholds.rust=0.5\
             &thresholds.typescript=0.5&thresholds.python=0.5\
             &providers.cohere.api_key_env=OTHER_KEY",
        ),
    ] {
        let (status, body) = post_settings_form(&app, section, form).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{section} {form}: {body}"
        );
        assert!(body.contains("Validation errors"), "{body}");
        if form.contains("bogus") {
            assert!(body.contains("bogus"), "{body}");
        }
        assert_eq!(
            fs::read(&config_path).unwrap(),
            before,
            "config.toml changed for {section} {form}"
        );
    }

    let (status, json) = get_settings_json(&app, "inference").await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["provider"], "auto");
}

#[tokio::test]
async fn contracts_dismiss_on_read_only_dashboard_returns_conflict() {
    let (_tmp, app, _ids) = seeded_app().await;

    let (status, json) = send_json(
        &app,
        "POST",
        "/api/v1/contracts/dismiss",
        Some(r#"{"violation_id":1}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"], "read_only");
    let message = json["message"].as_str().unwrap();
    assert!(message.contains("aether_contract_dismiss"), "{message}");
    assert!(!message.contains("--dismiss"), "{message}");
}
