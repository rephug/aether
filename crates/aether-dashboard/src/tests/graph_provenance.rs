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
    assert_eq!(orphan_metric["value"].as_f64(), Some(1.0), "{json}");
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
