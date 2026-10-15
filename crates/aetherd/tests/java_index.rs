use std::fs;
use std::path::Path;

use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord, TestIntentStore};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;

const MAIN: &str = "src/main/java/com/acme/shop";
const TEST: &str = "src/test/java/com/acme/shop";

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

    fs::write(workspace.join("pom.xml"), "<project></project>\n")?;
    fs::create_dir_all(workspace.join(MAIN))?;
    fs::create_dir_all(workspace.join(TEST))?;
    fs::write(
        workspace.join(MAIN).join("Store.java"),
        r#"package com.acme.shop;

public class Store {
    private final Repo repo;

    public Store(Repo repo) {
        this.repo = repo;
    }

    public String get(String key) {
        return repo.lookup(key);
    }
}
"#,
    )?;
    fs::write(
        workspace.join(MAIN).join("Repo.java"),
        r#"package com.acme.shop;

public class Repo implements Lookup {
    public String lookup(String key) {
        return key;
    }
}
"#,
    )?;
    fs::write(
        workspace.join(MAIN).join("Lookup.java"),
        r#"package com.acme.shop;

public interface Lookup {
    String lookup(String key);
}
"#,
    )?;
    fs::write(
        workspace.join(TEST).join("StoreTest.java"),
        r#"package com.acme.shop;

import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;

class StoreTest {
    /** Stored values come back. */
    @Test
    void getReturnsStoredValue() {
        Store store = new Store(new Repo());
        store.get("a");
    }

    @Nested
    class WhenEmpty {
        @Test
        void rejectsLookup() {}
    }

    @Nested
    class WhenClosed {
        @Test
        void rejectsLookup() {}
    }

    @Test
    void parses() {}

    @ParameterizedTest
    void parses(String input) {}
}
"#,
    )?;
    Ok(())
}

fn find<'a>(symbols: &'a [SymbolRecord], qualified_name: &str) -> &'a SymbolRecord {
    symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("missing indexed symbol {qualified_name}"))
}

#[test]
fn structural_index_stores_java_symbols_edges_and_test_intents()
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
    for file in [
        format!("{MAIN}/Store.java"),
        format!("{MAIN}/Repo.java"),
        format!("{MAIN}/Lookup.java"),
        format!("{TEST}/StoreTest.java"),
    ] {
        symbols.extend(store.list_symbols_for_file(&file)?);
    }
    assert!(symbols.iter().all(|symbol| symbol.language == "java"));

    let store_type = find(&symbols, "Store");
    let get = find(&symbols, "Store::get");
    let constructor = find(&symbols, "Store::Store");
    let repo = find(&symbols, "Repo");
    let lookup = find(&symbols, "Repo::lookup");
    let lookup_interface = find(&symbols, "Lookup");
    let test = find(&symbols, "StoreTest::getReturnsStoredValue");
    assert_eq!(store_type.kind, "class");
    assert_eq!(get.kind, "method");
    assert_eq!(lookup_interface.kind, "interface");

    // Edges resolve across files: a field's declared type qualifies the call, constructors
    // reference their parameter types, `implements` links to the interface, and a test's
    // local variable resolves to the class under test.
    let resolved = store
        .list_graph_dependency_edges()?
        .into_iter()
        .map(|edge| (edge.source_symbol_id, edge.target_symbol_id, edge.edge_kind))
        .collect::<Vec<_>>();
    for expected in [
        (&get.id, &lookup.id, "calls"),
        (&constructor.id, &repo.id, "type_ref"),
        (&repo.id, &lookup_interface.id, "implements"),
        (&test.id, &get.id, "calls"),
        (&test.id, &store_type.id, "calls"),
    ] {
        assert!(
            resolved.iter().any(|(source, target, kind)| {
                source == expected.0 && target == expected.1 && kind == expected.2
            }),
            "missing resolved edge {expected:?} in {resolved:?}"
        );
    }

    // Nested classes that reuse a test name keep separate intents, and overloads that share
    // a name and text collapse into one instead of failing the file.
    let intents = store.list_test_intents_for_file(&format!("{TEST}/StoreTest.java"))?;
    let mut names = intents
        .iter()
        .map(|intent| intent.test_name.as_str())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "WhenClosed::rejectsLookup",
            "WhenEmpty::rejectsLookup",
            "getReturnsStoredValue",
            "parses",
        ]
    );
    let stored = intents
        .iter()
        .find(|intent| intent.test_name == "getReturnsStoredValue")
        .expect("intent for getReturnsStoredValue");
    assert_eq!(stored.intent_text, "Stored values come back.");
    assert_eq!(stored.group_label.as_deref(), Some("StoreTest"));
    assert_eq!(stored.language, "java");
    assert_eq!(stored.symbol_id.as_deref(), Some(test.id.as_str()));

    Ok(())
}
