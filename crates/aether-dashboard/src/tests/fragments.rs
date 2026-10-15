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
async fn anatomy_file_fragment_keeps_symbols_from_every_layer_of_the_file() {
    // src/lib.rs holds demo::run (Core Logic) and demo::helper (Utilities), so the
    // file appears under two layers. Its detail must list both symbols, not only the
    // ones from whichever layer was processed last.
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
    assert!(
        body.contains("demo::run"),
        "missing Core Logic symbol: {body}"
    );
    assert!(
        body.contains("demo::helper"),
        "missing Utilities symbol: {body}"
    );
    assert!(body.contains("Run demo task"), "missing SIR intent: {body}");
    assert!(
        body.contains("Core Logic"),
        "missing Core Logic label: {body}"
    );
    assert!(
        body.contains("Utilities"),
        "missing Utilities label: {body}"
    );
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

#[tokio::test]
async fn settings_inference_fragment_renders_tiered_defaults_when_unset() {
    let (_tmp, app) = empty_app().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/settings/inference?provider=tiered")
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
    // With no [inference.tiered] stored, the form must still carry the tiered
    // inputs (at their defaults) so saving does not read them as 0.0 / false.
    assert!(body.contains("name=\"tiered.primary_threshold\" value=\"0.80\""));
    let retry_input = body
        .split("id=\"tiered.retry_with_fallback\"")
        .nth(1)
        .and_then(|rest| rest.split('>').next())
        .unwrap_or_default();
    assert!(retry_input.contains(" checked"), "{retry_input}");
}

#[tokio::test]
async fn wizard_detect_scans_workspace_root_whose_name_starts_with_dot() {
    let (_tmp, app) = empty_app().await;
    let workspace = tempfile::Builder::new()
        .prefix(".dotted-workspace-")
        .tempdir()
        .unwrap();
    let root = workspace.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();
    // Hidden and vendor directories below the root must still be skipped.
    fs::create_dir_all(root.join(".hidden")).unwrap();
    fs::write(root.join(".hidden/skipped.rs"), "fn skipped() {}\n").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/skipped.rs"), "fn skipped() {}\n").unwrap();

    let uri = format!(
        "/dashboard/frag/wizard/detect?workspace_path={}",
        crate::support::percent_encode(root.to_str().unwrap())
    );
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
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
    assert!(body.contains("Detected 3 files"), "body: {body}");
    assert!(body.contains("Rust<span"), "body: {body}");
    assert!(body.contains("(2)"), "body: {body}");
    assert!(
        !body.contains("No recognized source files found."),
        "body: {body}"
    );
}

/// A query value that would close an inline `<script>` element early if it
/// were embedded without script-context escaping.
const WIZARD_SCRIPT_BREAKOUT: &str = "</script><script>alert(document.domain)</script>";

/// Percent-encode a query value so arbitrary characters survive the request URI.
fn query_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// Decode the entities maud emits when it escapes attribute values.
fn html_attr_unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

async fn wizard_fragment_body(uri: &str) -> String {
    let response = crate::wizard_router()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

/// Parse the JSON string literal that follows `prefix` and ends at `terminator`.
fn script_string_after(body: &str, prefix: &str, terminator: &str) -> String {
    let start = body
        .find(prefix)
        .unwrap_or_else(|| panic!("missing {prefix:?} in {body}"))
        + prefix.len();
    let end = start + body[start..].find(terminator).unwrap();
    serde_json::from_str(&body[start..end]).unwrap()
}

#[tokio::test]
async fn wizard_steps_keep_query_values_inside_inline_script() {
    let payload = query_encode(WIZARD_SCRIPT_BREAKOUT);

    let environment = wizard_fragment_body(&format!(
        "/dashboard/frag/wizard/step/3?workspace_path={payload}"
    ))
    .await;
    let confirm = wizard_fragment_body(&format!(
        "/dashboard/frag/wizard/step/5?workspace_path={payload}&provider={payload}&api_key_env={payload}"
    ))
    .await;

    for (step, body) in [("environment", &environment), ("confirm", &confirm)] {
        assert_eq!(
            body.matches("<script").count(),
            1,
            "{step} step: a query value opened another script element:\n{body}"
        );
        assert!(
            !body.contains("</script><script>"),
            "{step} step: a query value closed the inline script:\n{body}"
        );
    }

    // The escaped literals must still decode to the original values in JS.
    assert_eq!(
        script_string_after(&environment, "var ws = ", ";\n"),
        WIZARD_SCRIPT_BREAKOUT
    );
    for key in ["workspacePath: ", "provider: ", "apiKeyEnv: "] {
        assert_eq!(
            script_string_after(&confirm, key, ",\n"),
            WIZARD_SCRIPT_BREAKOUT,
            "confirm step: {key}"
        );
    }
}

#[tokio::test]
async fn wizard_workspace_step_hx_vals_is_json_for_any_path() {
    for workspace in [
        r"C:\Users\dev\new-proj",
        r#"/srv/a"b/proj"#,
        r#"x","provider":"injected"#,
    ] {
        let body = wizard_fragment_body(&format!(
            "/dashboard/frag/wizard/step/2?workspace_path={}",
            query_encode(workspace)
        ))
        .await;
        let start = body
            .find("hx-vals=\"")
            .unwrap_or_else(|| panic!("missing hx-vals for {workspace:?} in {body}"))
            + "hx-vals=\"".len();
        let end = start + body[start..].find('"').unwrap();
        let hx_vals = html_attr_unescape(&body[start..end]);
        let parsed: Value = serde_json::from_str(&hx_vals)
            .unwrap_or_else(|err| panic!("hx-vals {hx_vals:?} is not valid JSON: {err}"));
        assert_eq!(
            parsed,
            serde_json::json!({ "workspace_path": workspace }),
            "hx-vals {hx_vals:?} did not round-trip {workspace:?}"
        );
    }
}
