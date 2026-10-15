use std::fs;
use std::path::Path;

use aether_core::Language;
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord, TestIntentStore};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;

const PROJECT: &str = "src/Acme.Shop";
const TESTS: &str = "tests/Acme.Shop.Tests";

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

    fs::create_dir_all(workspace.join(PROJECT))?;
    fs::create_dir_all(workspace.join(TESTS))?;
    fs::write(
        workspace.join(PROJECT).join("Store.cs"),
        r#"namespace Acme.Shop;

public class Store
{
    private readonly Repo _repo;

    public Store(Repo repo)
    {
        _repo = repo;
    }

    public string Get(string key)
    {
        return _repo.Lookup(key);
    }
}
"#,
    )?;
    fs::write(
        workspace.join(PROJECT).join("Repo.cs"),
        r#"namespace Acme.Shop;

public class Repo : ILookup
{
    public string Lookup(string key) => key;
}
"#,
    )?;
    fs::write(
        workspace.join(PROJECT).join("ILookup.cs"),
        r#"namespace Acme.Shop;

public interface ILookup
{
    string Lookup(string key);
}
"#,
    )?;
    fs::write(
        workspace.join(TESTS).join("StoreTests.cs"),
        r#"using Xunit;

namespace Acme.Shop.Tests;

public class StoreTests
{
    /// <summary>Stored values come back.</summary>
    [Fact]
    public void Get_ReturnsStoredValue()
    {
        var store = new Store(new Repo());
        store.Get("a");
    }

    public class WhenEmpty
    {
        [Fact]
        public void RejectsLookup() { }
    }

    public class WhenClosed
    {
        [Fact]
        public void RejectsLookup() { }
    }

    [Fact]
    public void Parses() { }

    [Theory]
    public void Parses(string input) { }
}
"#,
    )?;
    fs::write(
        workspace.join(TESTS).join("MathTests.cs"),
        r#"using Xunit;

namespace Acme.Shop.Tests;

public class AddTests
{
    [Fact]
    public void Works() { }
}

public class SubtractTests
{
    [Fact]
    public void Works() { }
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
fn csharp_serializes_with_its_stored_spelling() {
    assert_eq!(Language::CSharp.as_str(), "csharp");
    assert_eq!(
        serde_json::to_string(&Language::CSharp).expect("serialize"),
        "\"csharp\""
    );
    assert_eq!(
        serde_json::from_str::<Language>("\"csharp\"").expect("deserialize"),
        Language::CSharp
    );
}

#[test]
fn structural_index_stores_csharp_symbols_edges_and_test_intents()
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
        format!("{PROJECT}/Store.cs"),
        format!("{PROJECT}/Repo.cs"),
        format!("{PROJECT}/ILookup.cs"),
        format!("{TESTS}/StoreTests.cs"),
    ] {
        symbols.extend(store.list_symbols_for_file(&file)?);
    }
    assert!(symbols.iter().all(|symbol| symbol.language == "csharp"));

    let store_type = find(&symbols, "Store");
    let get = find(&symbols, "Store::Get");
    let constructor = find(&symbols, "Store::Store");
    let repo = find(&symbols, "Repo");
    let lookup = find(&symbols, "Repo::Lookup");
    let lookup_interface = find(&symbols, "ILookup");
    let test = find(&symbols, "StoreTests::Get_ReturnsStoredValue");
    assert_eq!(store_type.kind, "class");
    assert_eq!(get.kind, "method");
    assert_eq!(lookup_interface.kind, "interface");

    // Edges resolve across files: a field's declared type qualifies the call, constructors
    // reference their parameter types, the base list links to the interface, and a test's
    // `var` local resolves to the class it creates.
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
    let intents = store.list_test_intents_for_file(&format!("{TESTS}/StoreTests.cs"))?;
    let mut names = intents
        .iter()
        .map(|intent| intent.test_name.as_str())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "Get_ReturnsStoredValue",
            "Parses",
            "WhenClosed::RejectsLookup",
            "WhenEmpty::RejectsLookup",
        ]
    );
    // Sibling top-level test classes name their tests by full path, so both are stored.
    let mut sibling_names = store
        .list_test_intents_for_file(&format!("{TESTS}/MathTests.cs"))?
        .into_iter()
        .map(|intent| intent.test_name)
        .collect::<Vec<_>>();
    sibling_names.sort();
    assert_eq!(
        sibling_names,
        vec!["AddTests::Works", "SubtractTests::Works"]
    );

    let stored = intents
        .iter()
        .find(|intent| intent.test_name == "Get_ReturnsStoredValue")
        .expect("intent for Get_ReturnsStoredValue");
    assert_eq!(stored.intent_text, "Stored values come back.");
    assert_eq!(stored.group_label.as_deref(), Some("StoreTests"));
    assert_eq!(stored.language, "csharp");
    assert_eq!(stored.symbol_id.as_deref(), Some(test.id.as_str()));

    Ok(())
}
