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
