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
