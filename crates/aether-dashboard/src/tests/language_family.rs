use super::*;

fn method(id: &str, file_path: &str, language: &str, qualified_name: &str) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: file_path.to_owned(),
        language: language.to_owned(),
        kind: "method".to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{id}"),
        last_seen_at: 1_700_000_000,
    }
}

/// A Java and a TypeScript class both declare `UserService::getUser`; a Java method
/// and a top-level call in a JavaScript file call it. A second Java method calls
/// `Widget::render`, which only TypeScript declares.
async fn mixed_language_state() -> (TempDir, Arc<SharedState>) {
    let temp = TempDir::new().unwrap();
    let mut config = AetherConfig::default();
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).unwrap();

    let store = SqliteStore::open(temp.path()).unwrap();
    for record in [
        method(
            "java-get-user",
            "src/main/java/UserService.java",
            "java",
            "UserService::getUser",
        ),
        method(
            "ts-get-user",
            "web/userService.ts",
            "typescript",
            "UserService::getUser",
        ),
        method(
            "java-show",
            "src/main/java/UserController.java",
            "java",
            "UserController::show",
        ),
        method(
            "java-page",
            "src/main/java/UserController.java",
            "java",
            "UserController::page",
        ),
        method("ts-render", "web/widget.ts", "typescript", "Widget::render"),
        method("js-boot", "web/main.js", "javascript", "boot"),
    ] {
        store.upsert_symbol(record).unwrap();
    }
    store
        .upsert_edges(&[
            SymbolEdge {
                source_id: "java-show".to_owned(),
                target_qualified_name: "UserService::getUser".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/main/java/UserController.java".to_owned(),
            },
            SymbolEdge {
                source_id: "java-page".to_owned(),
                target_qualified_name: "Widget::render".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/main/java/UserController.java".to_owned(),
            },
            SymbolEdge {
                source_id: aether_core::file_source_id("web/main.js"),
                target_qualified_name: "UserService::getUser".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "web/main.js".to_owned(),
            },
        ])
        .unwrap();
    drop(store);

    let state = Arc::new(SharedState::open_readonly_async(temp.path()).await.unwrap());
    (temp, state)
}

#[tokio::test]
async fn dependency_edges_resolve_within_the_callers_language_family() {
    let (_temp, state) = mixed_language_state().await;

    let mut edges = crate::api::common::load_dependency_algo_edges(&state)
        .unwrap()
        .into_iter()
        .map(|edge| (edge.source_id, edge.target_id))
        .collect::<Vec<_>>();
    edges.sort();
    assert_eq!(
        edges,
        vec![
            (
                aether_core::file_source_id("web/main.js"),
                "ts-get-user".to_owned()
            ),
            ("java-show".to_owned(), "java-get-user".to_owned()),
        ]
    );
}

#[tokio::test]
async fn orphan_count_ignores_calls_from_another_language() {
    let (_temp, state) = mixed_language_state().await;
    let app = dashboard_router(state);

    // A Java call doesn't reach the TypeScript `Widget::render`, so it is an orphan
    // along with `boot`.
    let (status, json) = send_json(&app, "GET", "/api/v1/health-scorecard", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let orphans = json["data"]["metrics"]
        .as_array()
        .expect("metrics array")
        .iter()
        .find(|metric| metric["name"] == "orphan_count")
        .expect("orphan_count metric");
    assert_eq!(orphans["value"].as_f64(), Some(2.0), "{json}");
}
