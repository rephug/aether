use std::fs;
use std::path::Path;

use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;

/// A Java and a TypeScript class both declare `UserService::getUser`; a Python module
/// at the workspace root and a Go package both declare `config::load`.
const FILES: &[(&str, &str)] = &[
    (
        "src/main/java/com/acme/UserService.java",
        r#"package com.acme;

public class UserService {
    public String getUser(String id) {
        return id;
    }
}
"#,
    ),
    (
        "src/main/java/com/acme/UserController.java",
        r#"package com.acme;

public class UserController {
    private final UserService users;

    public UserController(UserService users) {
        this.users = users;
    }

    public String show(String id) {
        return users.getUser(id);
    }
}
"#,
    ),
    (
        "web/src/userService.ts",
        r#"export class UserService {
  getUser(id: string): string {
    return id;
  }
}
"#,
    ),
    (
        "config.py",
        r#"def load():
    return {}
"#,
    ),
    (
        "app.py",
        r#"import config


def main():
    return config.load()
"#,
    ),
    (
        "config/config.go",
        r#"package config

func load() map[string]string {
	return nil
}
"#,
    ),
    (
        "config/init.go",
        r#"package config

func Init() map[string]string {
	return load()
}
"#,
    ),
];

fn write_workspace(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;
    for (path, contents) in FILES {
        let path = workspace.join(path);
        fs::create_dir_all(path.parent().expect("file has a parent"))?;
        fs::write(path, contents)?;
    }
    Ok(())
}

fn find<'a>(symbols: &'a [SymbolRecord], language: &str, qualified_name: &str) -> &'a SymbolRecord {
    symbols
        .iter()
        .find(|symbol| symbol.language == language && symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("missing indexed {language} symbol {qualified_name}"))
}

#[test]
fn call_edges_resolve_only_within_the_callers_language_family()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_workspace(workspace)?;

    let config = IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        embeddings_only: false,
        force: false,
        full: false,
        deep: false,
        turbo_concurrency: None,
        dry_run: false,
        sir_concurrency: 2,
        lifecycle_logs: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        pause_flag: None,
    };
    run_initial_index_once(&config)?;

    let store = SqliteStore::open(workspace)?;
    let mut symbols = Vec::new();
    for (path, _) in FILES {
        symbols.extend(store.list_symbols_for_file(path)?);
    }

    // The same qualified names really are declared twice, in different languages.
    let java_get_user = find(&symbols, "java", "UserService::getUser");
    let ts_get_user = find(&symbols, "typescript", "UserService::getUser");
    let show = find(&symbols, "java", "UserController::show");
    let python_load = find(&symbols, "python", "config::load");
    let python_main = find(&symbols, "python", "app::main");
    let go_load = find(&symbols, "go", "config::load");
    let go_init = find(&symbols, "go", "config::Init");

    let resolved = store
        .list_graph_dependency_edges()?
        .into_iter()
        .filter(|edge| edge.edge_kind == "calls")
        .map(|edge| (edge.source_symbol_id, edge.target_symbol_id))
        .collect::<Vec<_>>();
    let reaches = |source: &SymbolRecord, target: &SymbolRecord| {
        resolved
            .iter()
            .any(|(from, to)| from == &source.id && to == &target.id)
    };
    assert!(reaches(show, java_get_user), "{resolved:?}");
    assert!(!reaches(show, ts_get_user), "{resolved:?}");
    assert!(reaches(python_main, python_load), "{resolved:?}");
    assert!(!reaches(python_main, go_load), "{resolved:?}");
    assert!(reaches(go_init, go_load), "{resolved:?}");
    assert!(!reaches(go_init, python_load), "{resolved:?}");

    // The denormalized neighbors written while indexing never link two families. (Only
    // absence is checked: a caller's rows are dropped when its callee's file is indexed
    // after it, which is tracked separately.)
    let neighbor_ids = |symbol: &SymbolRecord| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(store
            .get_symbol_neighbors(&symbol.id)?
            .into_iter()
            .map(|neighbor| neighbor.neighbor_id)
            .collect())
    };
    assert!(neighbor_ids(ts_get_user)?.is_empty());
    assert!(!neighbor_ids(show)?.contains(&ts_get_user.id));
    assert!(!neighbor_ids(python_main)?.contains(&go_load.id));
    assert!(!neighbor_ids(go_init)?.contains(&python_load.id));
    assert!(!neighbor_ids(go_load)?.contains(&python_main.id));
    assert!(!neighbor_ids(python_load)?.contains(&go_init.id));

    Ok(())
}
