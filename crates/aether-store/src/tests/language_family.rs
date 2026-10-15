use super::*;

fn symbol(id: &str, file_path: &str, language: &str, qualified_name: &str) -> SymbolRecord {
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

/// A Java and a TypeScript class both declare `UserService::getUser`, and a Python module
/// and a Go package both declare `config::load`. Callers in each language, a JavaScript
/// caller of the TypeScript class, a top-level call in a JavaScript file, and a Java call
/// to a name only TypeScript declares.
struct MixedWorkspace {
    java_get_user: SymbolRecord,
    java_controller: SymbolRecord,
    ts_get_user: SymbolRecord,
    js_render: SymbolRecord,
    python_load: SymbolRecord,
    python_main: SymbolRecord,
    go_load: SymbolRecord,
    go_init: SymbolRecord,
    ts_widget_render: SymbolRecord,
    java_page: SymbolRecord,
}

const JAVA_SERVICE: &str = "src/main/java/com/acme/UserService.java";
const JAVA_CONTROLLER: &str = "src/main/java/com/acme/UserController.java";
const TS_SERVICE: &str = "web/src/userService.ts";
const JS_RENDER: &str = "web/src/render.js";
const JS_MAIN: &str = "web/src/main.js";
const PY_CONFIG: &str = "tools/config.py";
const PY_APP: &str = "tools/app.py";
const GO_CONFIG: &str = "config/config.go";
const GO_INIT: &str = "config/init.go";

fn seed_mixed_workspace(store: &SqliteStore) -> MixedWorkspace {
    let workspace = MixedWorkspace {
        java_get_user: symbol(
            "java-get-user",
            JAVA_SERVICE,
            "java",
            "UserService::getUser",
        ),
        java_controller: symbol(
            "java-controller",
            JAVA_CONTROLLER,
            "java",
            "UserController::show",
        ),
        ts_get_user: symbol(
            "ts-get-user",
            TS_SERVICE,
            "typescript",
            "UserService::getUser",
        ),
        js_render: symbol("js-render", JS_RENDER, "javascript", "renderUser"),
        python_load: symbol("py-load", PY_CONFIG, "python", "config::load"),
        python_main: symbol("py-main", PY_APP, "python", "app::main"),
        go_load: symbol("go-load", GO_CONFIG, "go", "config::load"),
        go_init: symbol("go-init", GO_INIT, "go", "config::Init"),
        ts_widget_render: symbol(
            "ts-widget-render",
            TS_SERVICE,
            "typescript",
            "Widget::render",
        ),
        java_page: symbol("java-page", JAVA_CONTROLLER, "java", "UserController::page"),
    };
    for record in [
        &workspace.java_get_user,
        &workspace.java_controller,
        &workspace.ts_get_user,
        &workspace.js_render,
        &workspace.python_load,
        &workspace.python_main,
        &workspace.go_load,
        &workspace.go_init,
        &workspace.ts_widget_render,
        &workspace.java_page,
        &symbol("js-main-helper", JS_MAIN, "javascript", "boot"),
    ] {
        store.upsert_symbol(record.clone()).expect("upsert symbol");
    }
    store
        .upsert_edges(&[
            calls_edge(
                &workspace.java_controller.id,
                "UserService::getUser",
                JAVA_CONTROLLER,
            ),
            calls_edge(&workspace.js_render.id, "UserService::getUser", JS_RENDER),
            calls_edge(
                &aether_core::file_source_id(JS_MAIN),
                "UserService::getUser",
                JS_MAIN,
            ),
            calls_edge(&workspace.python_main.id, "config::load", PY_APP),
            calls_edge(&workspace.go_init.id, "config::load", GO_INIT),
            calls_edge(&workspace.java_page.id, "Widget::render", JAVA_CONTROLLER),
        ])
        .expect("upsert edges");
    workspace
}

fn neighbor_ids(store: &SqliteStore, symbol_id: &str, edge_type: &str) -> Vec<String> {
    store
        .get_symbol_neighbors_by_type(symbol_id, edge_type)
        .expect("load neighbors")
        .into_iter()
        .map(|neighbor| neighbor.neighbor_id)
        .collect()
}

fn assert_family_neighbors(store: &SqliteStore, workspace: &MixedWorkspace) {
    assert_eq!(
        neighbor_ids(store, &workspace.java_controller.id, "calls"),
        vec![workspace.java_get_user.id.clone()]
    );
    assert_eq!(
        neighbor_ids(store, &workspace.java_get_user.id, "called_by"),
        vec![workspace.java_controller.id.clone()]
    );
    assert_eq!(
        neighbor_ids(store, &workspace.ts_get_user.id, "called_by"),
        vec![workspace.js_render.id.clone()]
    );
    assert_eq!(
        neighbor_ids(store, &workspace.python_load.id, "called_by"),
        vec![workspace.python_main.id.clone()]
    );
    assert_eq!(
        neighbor_ids(store, &workspace.go_load.id, "called_by"),
        vec![workspace.go_init.id.clone()]
    );
    assert!(neighbor_ids(store, &workspace.java_page.id, "calls").is_empty());
    assert!(neighbor_ids(store, &workspace.ts_widget_render.id, "called_by").is_empty());
}

#[test]
fn graph_dependency_edges_resolve_within_the_source_language_family() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let workspace = seed_mixed_workspace(&store);

    let mut edges = store
        .list_graph_dependency_edges()
        .expect("list graph dependency edges")
        .into_iter()
        .map(|edge| (edge.source_symbol_id, edge.target_symbol_id))
        .collect::<Vec<_>>();
    edges.sort();
    let mut expected = vec![
        (
            workspace.java_controller.id.clone(),
            workspace.java_get_user.id.clone(),
        ),
        (
            workspace.js_render.id.clone(),
            workspace.ts_get_user.id.clone(),
        ),
        (
            aether_core::file_source_id(JS_MAIN),
            workspace.ts_get_user.id.clone(),
        ),
        (
            workspace.python_main.id.clone(),
            workspace.python_load.id.clone(),
        ),
        (workspace.go_init.id.clone(), workspace.go_load.id.clone()),
    ];
    expected.sort();
    assert_eq!(edges, expected);

    assert!(
        store
            .has_dependency_between_files(JS_RENDER, TS_SERVICE)
            .expect("js and ts dependency")
    );
    assert!(
        !store
            .has_dependency_between_files(JAVA_CONTROLLER, TS_SERVICE)
            .expect("java and ts dependency")
    );
    assert!(
        !store
            .has_dependency_between_files(PY_APP, GO_CONFIG)
            .expect("python and go dependency")
    );
}

#[test]
fn symbol_neighbors_link_only_symbols_of_one_language_family() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let workspace = seed_mixed_workspace(&store);

    for file in [JAVA_CONTROLLER, JS_RENDER, PY_APP, GO_INIT] {
        store
            .populate_symbol_neighbors(file)
            .expect("populate symbol neighbors");
    }
    assert_family_neighbors(&store, &workspace);

    // The migration-time rebuild of every neighbor row applies the same rule.
    {
        let conn = store.conn.lock().unwrap();
        crate::schema::rebuild_symbol_neighbors(&conn).expect("rebuild symbol neighbors");
    }
    assert_family_neighbors(&store, &workspace);
}

#[tokio::test]
async fn sqlite_graph_store_follows_edges_within_one_language_family() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let graph = SqliteGraphStore::open(temp.path()).expect("open sqlite graph store");
    let workspace = seed_mixed_workspace(&store);
    let python_caller = symbol("py-user", PY_APP, "python", "app::show_user");
    store
        .upsert_symbol(python_caller.clone())
        .expect("upsert python caller");
    store
        .upsert_edges(&[calls_edge(
            &python_caller.id,
            "UserService::getUser",
            PY_APP,
        )])
        .expect("upsert python edge");

    // A Python call to a name only Java and TypeScript declare reaches neither, so the
    // Python caller isn't a caller of that name.
    let callers = graph
        .get_callers("UserService::getUser")
        .await
        .expect("get callers")
        .into_iter()
        .map(|caller| caller.id)
        .collect::<Vec<_>>();
    assert_eq!(
        callers,
        vec![
            workspace.java_controller.id.clone(),
            workspace.js_render.id.clone(),
        ]
    );

    let chain = graph
        .get_call_chain(&workspace.java_controller.id, 2)
        .await
        .expect("get call chain");
    assert_eq!(chain.len(), 1);
    assert_eq!(
        chain[0]
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        vec![workspace.java_get_user.id.as_str()]
    );

    let stats = store
        .sync_graph_for_file(&graph, JAVA_CONTROLLER)
        .await
        .expect("sync java controller");
    assert_eq!(stats.resolved_edges, 1);
    assert_eq!(stats.unresolved_edges, 1);
}
