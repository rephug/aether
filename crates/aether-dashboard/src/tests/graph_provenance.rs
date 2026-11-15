use std::collections::BTreeSet;

use super::*;

async fn duplicated_project_symbols() -> (TempDir, Arc<SharedState>) {
    let temp = TempDir::new().unwrap();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    for (id, file, name) in [
        ("target-a", "alpha/dep.ts", "helper"),
        ("target-b", "beta/dep.ts", "helper"),
        ("caller-a", "alpha/main.ts", "callerAlpha"),
        ("caller-b", "beta/main.ts", "callerBeta"),
        ("unproven-caller", "unrelated/main.ts", "rawCaller"),
        ("lonely-target", "lonely/dep.ts", "onlyElsewhere"),
    ] {
        let path = temp.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("export function {name}() {{}}\n")).unwrap();
        store
            .upsert_symbol(SymbolRecord {
                id: id.to_owned(),
                file_path: file.to_owned(),
                language: "typescript".to_owned(),
                kind: "function".to_owned(),
                qualified_name: name.to_owned(),
                signature_fingerprint: format!("sig-{id}"),
                last_seen_at: 1_700_000_000,
            })
            .unwrap();
    }
    let edge = |source: &str, file: &str, name: &str, target: Option<&str>| SymbolEdge {
        source_id: source.to_owned(),
        file_path: file.to_owned(),
        target_qualified_name: name.to_owned(),
        edge_kind: EdgeKind::Calls,
        target_symbol_id: target.map(str::to_owned),
        target_file_path: None,
    };
    store
        .upsert_edges(&[
            edge("caller-a", "alpha/main.ts", "helper", Some("target-a")),
            edge("caller-b", "beta/main.ts", "helper", Some("target-b")),
            edge("unproven-caller", "unrelated/main.ts", "helper", None),
            edge(
                "unproven-caller",
                "unrelated/main.ts",
                "onlyElsewhere",
                None,
            ),
        ])
        .unwrap();
    for file in ["alpha/main.ts", "beta/main.ts", "unrelated/main.ts"] {
        store.populate_symbol_neighbors(file).unwrap();
    }
    drop(store);
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    (temp, state)
}

#[tokio::test]
async fn graph_algorithms_and_orphan_metrics_ignore_unproven_same_name_references() {
    let (_temp, state) = duplicated_project_symbols().await;
    let edges = crate::api::common::load_dependency_algo_edges(&state)
        .unwrap()
        .into_iter()
        .map(|edge| (edge.source_id, edge.target_id))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        edges,
        BTreeSet::from([
            ("caller-a".to_owned(), "target-a".to_owned()),
            ("caller-b".to_owned(), "target-b".to_owned()),
        ])
    );
    let app = dashboard_router(state);
    let (status, json) = send_json(&app, "GET", "/api/v1/health-scorecard", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let orphan_metric = json["data"]["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|metric| metric["name"] == "orphan_count")
        .unwrap();
    assert_eq!(orphan_metric["value"].as_f64(), Some(2.0), "{json}");
}

#[tokio::test]
async fn selected_symbol_graph_and_search_do_not_attach_another_projects_callers() {
    let (_temp, state) = duplicated_project_symbols().await;
    let app = dashboard_router(state);
    let (status, graph) = send_json(&app, "GET", "/api/v1/graph?root=target-a&depth=1", None).await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    let nodes = graph["data"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(nodes, BTreeSet::from(["target-a", "caller-a"]), "{graph}");
    assert_eq!(
        graph["data"]["edges"].as_array().unwrap().len(),
        1,
        "{graph}"
    );
    assert_eq!(graph["data"]["edges"][0]["source"], "caller-a");
    assert_eq!(graph["data"]["edges"][0]["target"], "target-a");

    let (status, search) = send_json(
        &app,
        "GET",
        "/api/v1/search?q=helper&mode=lexical&limit=20",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{search}");
    for (target, caller) in [("target-a", "caller-a"), ("target-b", "caller-b")] {
        let result = search["data"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["symbol_id"] == target)
            .unwrap();
        let related = result["related_symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|related| related["symbol_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(related, BTreeSet::from([caller]), "{result}");
    }
}

#[tokio::test]
async fn context_builder_keeps_the_selected_declarations_callers_separate() {
    let (_temp, state) = duplicated_project_symbols().await;
    let app = dashboard_router(state);
    let request = serde_json::json!({
        "targets": ["alpha/dep.ts"], "budget": 12000,
        "layers": { "source": false, "sir": false, "graph": true }
    });
    let (status, response) = send_json(
        &app,
        "POST",
        "/api/v1/context/build",
        Some(&request.to_string()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let content = response["data"]["content"].as_str().unwrap();
    assert!(content.contains("callerAlpha"), "{content}");
    assert!(!content.contains("callerBeta"), "{content}");
    assert!(!content.contains("rawCaller"), "{content}");
}

#[tokio::test]
async fn dashboard_reports_partial_observed_graph_without_absence_health_grades() {
    let (_temp, state) = duplicated_project_symbols().await;
    let app = dashboard_router(state);
    for endpoint in ["/api/v1/xray", "/api/v1/health-scorecard", "/api/v1/health"] {
        let (status, json) = send_json(&app, "GET", endpoint, None).await;
        assert_eq!(status, StatusCode::OK, "{endpoint}: {json}");
        let coverage = &json["data"]["graph_coverage"];
        assert_eq!(coverage["status"], "incomplete", "{endpoint}: {json}");
        assert_eq!(coverage["whole_program_completeness"], "unknown");
        assert_eq!(coverage["call_edges"], 4);
        assert_eq!(coverage["resolved"], 2);
        assert_eq!(coverage["unresolved"], 2);
        assert!(coverage["note"].as_str().unwrap().contains("not recall"));
        if endpoint.ends_with("health-scorecard") {
            for name in ["orphan_count", "graph_connectivity"] {
                let metric = json["data"]["metrics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|metric| metric["name"] == name)
                    .unwrap();
                assert_eq!(metric["status"], "unknown", "{json}");
                assert!(metric["trend"].as_array().unwrap().is_empty(), "{json}");
            }
        }
    }
}

#[tokio::test]
async fn empty_graph_is_unknown_even_without_stored_unresolved_references() {
    let (_temp, app) = empty_app().await;
    let (status, json) = send_json(&app, "GET", "/api/v1/xray", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["graph_coverage"]["status"], "unknown");
    assert_eq!(json["data"]["graph_coverage"]["call_edges"], 0);
    assert_eq!(
        json["data"]["graph_coverage"]["whole_program_completeness"],
        "unknown"
    );
    assert_eq!(
        json["data"]["metrics"]["graph_connectivity"]["not_computed"],
        true
    );
    assert!(json["data"]["metrics"]["graph_connectivity"]["value"].is_null());
}

#[tokio::test]
async fn health_and_xray_fragments_explain_observed_graph_limits() {
    let (_temp, app, _) = seeded_app().await;
    for endpoint in ["/dashboard/frag/health", "/dashboard/frag/xray"] {
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
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("May indicate dead code"), "{text}");
        assert!(text.contains("Observed"), "{text}");
        if endpoint.ends_with("health") {
            assert!(text.contains("unused code"), "{text}");
        } else {
            assert!(text.contains("xray-graph-coverage"), "{text}");
        }
    }
}

#[tokio::test]
async fn failed_graph_queries_are_unavailable_instead_of_empty_or_zero() {
    let (temp, state) = duplicated_project_symbols().await;
    // Simulate an unavailable graph relation while symbol metadata remains usable.
    let conn = Connection::open(temp.path().join(".aether/meta.sqlite")).unwrap();
    conn.execute("DROP TABLE symbol_edges", []).unwrap();
    drop(conn);
    let app = dashboard_router(state);
    let (status, json) = send_json(&app, "GET", "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        json["data"]["graph_coverage"]["status"], "unavailable",
        "{json}"
    );
    assert!(json["data"]["graph_coverage"]["call_edges"].is_null());
    assert!(
        json["data"]["graph_coverage"]["note"]
            .as_str()
            .unwrap()
            .contains("symbol_edges"),
        "{json}"
    );
    assert!(json["data"]["dimensions"]["graph_connectivity"].is_null());
    for endpoint in ["/api/v1/xray", "/api/v1/health-scorecard"] {
        let (status, json) = send_json(&app, "GET", endpoint, None).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{endpoint}: {json}"
        );
    }
}

#[tokio::test]
async fn stale_proof_receipts_block_coverage_and_already_open_analysis_handle() {
    use aether_analysis::{
        HealthAnalyzer, HealthRequest, ObservedGraphCoverage, ObservedGraphStatus,
    };
    use aether_store::{GraphStore, ResolvedEdge, SurrealGraphStore};

    let temp = TempDir::new().unwrap();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Surreal;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    let graph = SurrealGraphStore::open(temp.path()).await.unwrap();
    for id in ["caller", "helper"] {
        let symbol = SymbolRecord {
            id: id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: id.to_owned(),
            signature_fingerprint: id.to_owned(),
            last_seen_at: 1,
        };
        store.upsert_symbol(symbol.clone()).unwrap();
        graph.upsert_symbol_node(&symbol).await.unwrap();
    }
    graph
        .upsert_edge(&ResolvedEdge {
            source_id: "caller".to_owned(),
            target_id: "helper".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_owned(),
        })
        .await
        .unwrap();
    let analyzer = HealthAnalyzer::new(temp.path()).unwrap();
    let before = analyzer
        .analyze_with_handles(&HealthRequest::default(), &store, Some(&graph))
        .await
        .unwrap();
    assert!(before.analysis_available);
    assert_eq!(before.analysis.total_edges, 1);

    let conn = Connection::open(temp.path().join(".aether/meta.sqlite")).unwrap();
    let original_proof: i64 = conn
        .query_row(
            "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE schema_version SET version = 0 WHERE component = 'rust_target_provenance'",
        [],
    )
    .unwrap();
    let coverage = ObservedGraphCoverage::from_store(&store);
    assert_eq!(coverage.status, ObservedGraphStatus::Unavailable);
    assert!(coverage.call_edges.is_none());
    assert!(coverage.note.contains("reindex"));
    let after = analyzer
        .analyze_with_handles(&HealthRequest::default(), &store, Some(&graph))
        .await
        .unwrap();
    assert!(!after.analysis_available, "{after:?}");
    assert_eq!(
        after.graph_coverage.status,
        ObservedGraphStatus::Unavailable
    );
    assert!(after.critical_symbols.is_empty());
    assert!(after.notes.iter().any(|note| note.contains("reindex")));
    let centrality = analyzer
        .centrality_by_file_with_handles(&store, &graph)
        .await
        .unwrap();
    assert!(centrality.files.is_empty());
    assert!(centrality.notes.iter().any(|note| note.contains("reindex")));
    conn.execute(
        "UPDATE schema_version SET version = ?1 WHERE component = 'rust_target_provenance'",
        [original_proof],
    )
    .unwrap();
    conn.execute(
        "UPDATE schema_version SET version = 0 WHERE component = 'graph_target_provenance'",
        [],
    )
    .unwrap();
    assert!(store.has_graph_provenance_schema().unwrap());
    let pending_projection = analyzer
        .analyze_with_handles(&HealthRequest::default(), &store, Some(&graph))
        .await
        .unwrap();
    assert!(
        !pending_projection.analysis_available,
        "{pending_projection:?}"
    );
    assert_eq!(
        pending_projection.graph_coverage.status,
        ObservedGraphStatus::Unavailable
    );
    assert!(
        pending_projection
            .notes
            .iter()
            .any(|note| note.contains("reindex"))
    );
    let community_err = aether_analysis::production_community_assignments(&store, &graph, 1.0)
        .await
        .expect_err("production community gate");
    assert!(
        community_err.to_string().contains("reindex"),
        "{community_err}"
    );

    let drift = aether_analysis::DriftAnalyzer::new(temp.path()).unwrap();
    let state_before = store.get_drift_analysis_state().unwrap();
    let result_count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM drift_results", [], |row| row.get(0))
        .unwrap();
    let err = drift
        .report_with_graph(&graph, aether_analysis::DriftReportRequest::default())
        .expect_err("drift report gate");
    assert!(err.to_string().contains("reindex"), "{err}");
    let err = drift
        .communities_with_graph(&graph, aether_analysis::CommunitiesRequest { format: None })
        .expect_err("drift communities gate");
    assert!(err.to_string().contains("reindex"), "{err}");
    assert_eq!(store.get_drift_analysis_state().unwrap(), state_before);
    let result_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM drift_results", [], |row| row.get(0))
        .unwrap();
    assert_eq!(result_count_after, result_count_before);
}

#[tokio::test]
async fn warm_health_score_cache_does_not_hide_pending_native_projection() {
    let temp = TempDir::new().unwrap();
    seed_workspace(temp.path());
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Surreal;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).unwrap();
    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    let warm = crate::api::health_score::load_health_score_report(&state)
        .await
        .unwrap();
    assert!(state.caches.health_score_report.read().unwrap().is_some());
    let conn = Connection::open(temp.path().join(".aether/meta.sqlite")).unwrap();
    let native_receipt: i64 = conn
        .query_row(
            "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute(
        "UPDATE schema_version SET version = 0 WHERE component = 'graph_target_provenance'",
        [],
    )
    .unwrap();
    let err = crate::api::health_score::load_health_score_report(&state)
        .await
        .expect_err("do not serve warm stale score");
    assert!(err.contains("reindex"), "{err}");

    let app = dashboard_router(state.clone());
    let (status, json) = send_json(&app, "GET", "/api/v1/health-score", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{json}");
    assert!(json.to_string().contains("reindex"), "{json}");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard/frag/health-score")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Failed to load health score"), "{text}");
    assert!(text.contains("reindex"), "{text}");
    assert!(!text.contains("Current workspace health"), "{text}");
    assert!(!text.contains("data-health-score-trend"), "{text}");

    conn.execute(
        "UPDATE schema_version SET version = ?1 WHERE component = 'graph_target_provenance'",
        [native_receipt],
    )
    .unwrap();
    let restored = crate::api::health_score::load_health_score_report(&state)
        .await
        .unwrap();
    assert_eq!(
        restored, warm,
        "restoring readiness keeps the valid warm score unchanged"
    );
}
