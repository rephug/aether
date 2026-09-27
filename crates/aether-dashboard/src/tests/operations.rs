//! Operations page tests (batch, continuous, task context, context export, presets,
//! fingerprint history, seismograph): API shapes and fragment rendering.

use super::*;

#[tokio::test]
async fn batch_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/batch-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["has_data"].as_bool().unwrap());
    assert!(json["data"]["written_requests"].is_number());
    assert!(json["data"]["skipped_requests"].is_number());
    assert!(json["data"]["batch_files"].is_array());
}

#[tokio::test]
async fn batch_api_empty_workspace_returns_no_data() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/batch-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["data"]["has_data"].as_bool().unwrap());
}

#[tokio::test]
async fn batch_fragment_renders_status_card() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/batch")
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
    assert!(body.contains("Batch"));
    assert!(body.contains("Requests Written"));
}

#[tokio::test]
async fn batch_fragment_empty_shows_empty_state() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/batch")
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
    assert!(body.contains("No batch runs yet"));
}

#[tokio::test]
async fn continuous_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/continuous-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["has_data"].as_bool().unwrap());
    assert_eq!(json["data"]["total_symbols"].as_u64().unwrap(), 3);
    assert!(json["data"]["score_bands"].is_object());
    assert!(json["data"]["most_stale"].is_object());
}

#[tokio::test]
async fn continuous_api_empty_workspace_returns_no_data() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/continuous-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["data"]["has_data"].as_bool().unwrap());
}

#[tokio::test]
async fn continuous_fragment_renders_staleness_distribution() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/continuous")
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
    assert!(body.contains("Staleness Distribution"));
    assert!(body.contains("demo::run"));
}

#[tokio::test]
async fn continuous_fragment_empty_shows_empty_state() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/continuous")
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
    assert!(body.contains("No continuous monitor data"));
}

#[tokio::test]
async fn task_context_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/task-history?limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["has_history"].as_bool().unwrap());
    assert_eq!(json["data"]["total_entries"].as_u64().unwrap(), 1);
    let entry = &json["data"]["entries"][0];
    assert_eq!(
        entry["task_description"].as_str().unwrap(),
        "Fix the login bug in auth module"
    );
    assert_eq!(entry["symbol_count"].as_i64().unwrap(), 2);
    assert_eq!(entry["file_count"].as_u64().unwrap(), 2);
    assert!(entry["budget_pct"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn task_context_api_empty_returns_no_history() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/task-history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["data"]["has_history"].as_bool().unwrap());
}

#[tokio::test]
async fn task_context_fragment_renders_history_table() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/task-context")
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
    assert!(body.contains("Fix the login bug"));
    assert!(body.contains("fix/auth-login"));
}

#[tokio::test]
async fn task_context_fragment_empty_shows_empty_state() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/task-context")
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
    assert!(body.contains("No task context resolutions yet"));
}

// ── Tests for Phase 9.1 Part A pages 4-6 ──────────────────────────────

#[tokio::test]
async fn context_export_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/context-targets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["available_files"].is_array());
    assert!(json["data"]["available_presets"].is_array());
    assert!(json["data"]["formats"].is_array());
    assert_eq!(json["data"]["default_budget"].as_u64().unwrap(), 32_000);
    // Should have at least the seeded file paths
    assert!(
        !json["data"]["available_files"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Should have built-in presets
    assert!(json["data"]["available_presets"].as_array().unwrap().len() >= 4);
}

#[tokio::test]
async fn context_export_fragment_renders_builder() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/context-export")
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
    assert!(body.contains("Context Export"));
    assert!(body.contains("ctx-target"));
    assert!(body.contains("ctx-budget"));
    assert!(body.contains("Copy Command"));
}

#[tokio::test]
async fn context_export_fragment_empty_shows_empty_state() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/context-export")
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
    assert!(body.contains("No symbols indexed yet"));
}

#[tokio::test]
async fn presets_api_returns_builtin_presets() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/presets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["presets"].is_array());
    let presets = json["data"]["presets"].as_array().unwrap();
    assert!(presets.len() >= 4);
    let names: Vec<&str> = presets.iter().filter_map(|p| p["name"].as_str()).collect();
    assert!(names.contains(&"quick"));
    assert!(names.contains(&"review"));
    assert!(names.contains(&"deep"));
    assert!(names.contains(&"overview"));
}

#[tokio::test]
async fn presets_fragment_renders_preset_cards() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/presets")
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
    assert!(body.contains("quick"));
    assert!(body.contains("built-in"));
    assert!(body.contains("Create Preset"));
}

#[tokio::test]
async fn fingerprint_summary_api_returns_expected_fields() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/fingerprint-summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["data"]["has_data"].as_bool().unwrap());
    assert_eq!(json["data"]["total_recent"].as_u64().unwrap(), 2);
    assert!(json["data"]["trigger_breakdown"].is_object());
    assert!(json["data"]["top_changed_symbols"].is_array());
    assert!(json["data"]["recent_changes"].is_array());
}

#[tokio::test]
async fn fingerprint_summary_api_empty_returns_no_data() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/fingerprint-summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(!json["data"]["has_data"].as_bool().unwrap());
}

#[tokio::test]
async fn fingerprint_history_api_returns_timeline() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/fingerprint-history?symbol_id=sym-demo-run")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["data"]["symbol_id"].as_str().unwrap(), "sym-demo-run");
    assert!(json["data"]["entries"].is_array());
    assert_eq!(json["data"]["entries"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn fingerprint_fragment_renders_summary() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/fingerprint")
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
    assert!(body.contains("Trigger Breakdown"));
    assert!(body.contains("Recent Changes"));
    assert!(body.contains("demo::run"));
}

#[tokio::test]
async fn fingerprint_fragment_empty_shows_empty_state() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/fingerprint")
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
    assert!(body.contains("No fingerprint history yet"));
}

#[tokio::test]
async fn fingerprint_fragment_with_symbol_shows_timeline() {
    let (_tmp, app, _ids) = seeded_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/fingerprint?symbol_id=sym-demo-run")
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
    assert!(body.contains("Timeline for"));
    assert!(body.contains("demo::run"));
    assert!(body.contains("abc123def456"));
}

// ─── Phase 10.4: Seismograph API tests ───────────────────────────────

#[tokio::test]
async fn seismograph_timeline_returns_ok() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/seismograph-timeline")
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
}

#[tokio::test]
async fn seismograph_plates_returns_ok() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/seismograph-plates")
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
}

#[tokio::test]
async fn seismograph_gauge_returns_ok() {
    let (_tmp, app) = empty_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/seismograph-gauge")
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
}
