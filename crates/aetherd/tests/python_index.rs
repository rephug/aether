use std::fs;
use std::path::Path;

use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord, TestIntentStore};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use tempfile::tempdir;

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

    fs::write(
        workspace.join("pyproject.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
    )?;
    fs::create_dir_all(workspace.join("shop"))?;
    fs::write(
        workspace.join("shop/__init__.py"),
        r#"def configure():
    return None
"#,
    )?;
    fs::write(
        workspace.join("shop/pricing.py"),
        r#"def quote(order):
    return round_total(order)


def round_total(order):
    return order


def discount(order):
    return order
"#,
    )?;
    fs::write(
        workspace.join("shop/orders.py"),
        r#"import shop.pricing as pricing
from shop import configure
from shop.pricing import quote
from .pricing import round_total as rounder


class Order:
    def total(self):
        return self.subtotal()

    def subtotal(self):
        return quote(self)


def place():
    configure()
    order = Order()
    rounder(order)
    " ".join([])
    return order.total()


def reprice(order):
    return pricing.discount(order)
"#,
    )?;
    // A Rust crate in the same workspace: its top-level functions are qualified by their
    // bare names, which a Python method call on an unknown value must not reach.
    fs::create_dir_all(workspace.join("native/src"))?;
    fs::write(
        workspace.join("native/src/lib.rs"),
        "pub fn join() {}\n\npub fn total() {}\n",
    )?;
    fs::create_dir_all(workspace.join("tests"))?;
    fs::write(
        workspace.join("tests/test_orders.py"),
        r#"from shop.orders import place


def test_place_returns_an_order():
    """placing an order returns it"""
    assert place() is not None
"#,
    )?;
    Ok(())
}

fn indexer_config(workspace: &Path) -> IndexerConfig {
    IndexerConfig {
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
    }
}

fn find<'a>(symbols: &'a [SymbolRecord], qualified_name: &str) -> &'a SymbolRecord {
    symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified_name)
        .unwrap_or_else(|| panic!("missing indexed symbol {qualified_name}"))
}

#[test]
fn structural_index_resolves_python_call_edges() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_workspace(workspace)?;

    run_initial_index_once(&indexer_config(workspace))?;

    let store = SqliteStore::open(workspace)?;
    let mut symbols = Vec::new();
    for file in [
        "shop/__init__.py",
        "shop/pricing.py",
        "shop/orders.py",
        "tests/test_orders.py",
    ] {
        symbols.extend(store.list_symbols_for_file(file)?);
    }
    assert!(symbols.iter().all(|symbol| symbol.language == "python"));
    let rust_symbols = store.list_symbols_for_file("native/src/lib.rs")?;

    let configure = find(&symbols, "shop::configure");
    let quote = find(&symbols, "shop.pricing::quote");
    let round_total = find(&symbols, "shop.pricing::round_total");
    let discount = find(&symbols, "shop.pricing::discount");
    let order = find(&symbols, "shop.orders::Order");
    let total = find(&symbols, "shop.orders::Order::total");
    let subtotal = find(&symbols, "shop.orders::Order::subtotal");
    let place = find(&symbols, "shop.orders::place");
    let reprice = find(&symbols, "shop.orders::reprice");
    let test = find(&symbols, "tests.test_orders::test_place_returns_an_order");
    let rust_join = find(&rust_symbols, "join");
    let rust_total = find(&rust_symbols, "total");
    assert_eq!(total.kind, "method");
    assert_eq!(place.kind, "function");

    let resolved = store
        .list_graph_dependency_edges()?
        .into_iter()
        .filter(|edge| edge.edge_kind == "calls")
        .map(|edge| (edge.source_symbol_id, edge.target_symbol_id))
        .collect::<Vec<_>>();
    let callees = |source: &SymbolRecord| {
        let mut callees = resolved
            .iter()
            .filter(|(edge_source, _)| *edge_source == source.id)
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>();
        callees.sort();
        callees
    };
    let ids = |records: &[&SymbolRecord]| {
        let mut ids = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };

    // Within a file: a function, a method through `self`.
    assert_eq!(callees(quote), ids(&[round_total]));
    assert_eq!(callees(total), ids(&[subtotal]));
    // Across files: `from shop.pricing import quote`, a package's own `__init__.py` function,
    // a class constructed in the same file, and a relative import's alias. `" ".join(...)` and
    // `order.total()` have no target, so the Rust `join` and `total` stay unreached.
    assert_eq!(callees(subtotal), ids(&[quote]));
    assert_eq!(callees(place), ids(&[configure, order, round_total]));
    // A module alias, `import shop.pricing as pricing`.
    assert_eq!(callees(reprice), ids(&[discount]));
    // A test outside the package.
    assert_eq!(callees(test), ids(&[place]));
    assert!(
        resolved
            .iter()
            .all(|(_, target)| *target != rust_join.id && *target != rust_total.id),
        "a Python call resolved to a Rust function: {resolved:?}"
    );

    let intents = store.list_test_intents_for_file("tests/test_orders.py")?;
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].test_name, "test_place_returns_an_order");
    assert_eq!(intents[0].intent_text, "placing an order returns it");
    assert_eq!(intents[0].language, "python");
    assert_eq!(intents[0].symbol_id.as_deref(), Some(test.id.as_str()));

    Ok(())
}

/// A package that re-exports its API from submodules, and a `src/` layout whose tests sit
/// outside `src/`.
#[test]
fn structural_index_resolves_python_calls_through_reexports_and_src_layouts()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_workspace(workspace)?;
    for (path, source) in [
        (
            "kit/__init__.py",
            "from .text import *\nfrom .numbers import scale as resize\n",
        ),
        (
            "kit/text.py",
            "__all__ = [\"slug\"]\n\n\ndef slug(value):\n    return _clean(value)\n\n\ndef _clean(value):\n    return value\n",
        ),
        ("kit/numbers.py", "def scale(value):\n    return value\n"),
        (
            "tests/test_kit.py",
            "import kit\nfrom kit import slug, resize\n\n\ndef test_kit():\n    slug(\"a\")\n    resize(1)\n    kit.slug(\"b\")\n",
        ),
        (
            "app/src/billing/__init__.py",
            "from .invoice import total\n",
        ),
        (
            "app/src/billing/invoice.py",
            "class Invoice:\n    def amount(self):\n        return 1\n\n\ndef total():\n    return Invoice().amount()\n",
        ),
        (
            "app/tests/test_billing.py",
            "from billing import total\nfrom billing.invoice import Invoice\n\n\ndef test_total():\n    total()\n    Invoice.amount(None)\n",
        ),
    ] {
        let full_path = workspace.join(path);
        fs::create_dir_all(full_path.parent().expect("parent"))?;
        fs::write(full_path, source)?;
    }
    run_initial_index_once(&indexer_config(workspace))?;

    let store = SqliteStore::open(workspace)?;
    let mut symbols = Vec::new();
    for file in [
        "kit/text.py",
        "kit/numbers.py",
        "tests/test_kit.py",
        "app/src/billing/invoice.py",
        "app/tests/test_billing.py",
    ] {
        symbols.extend(store.list_symbols_for_file(file)?);
    }
    let slug = find(&symbols, "kit.text::slug");
    let scale = find(&symbols, "kit.numbers::scale");
    let test_kit = find(&symbols, "tests.test_kit::test_kit");
    let total = find(&symbols, "app.src.billing.invoice::total");
    let amount = find(&symbols, "app.src.billing.invoice::Invoice::amount");
    let test_total = find(&symbols, "app.tests.test_billing::test_total");

    let resolved = store
        .list_graph_dependency_edges()?
        .into_iter()
        .filter(|edge| edge.edge_kind == "calls")
        .collect::<Vec<_>>();
    let callees = |source: &SymbolRecord| {
        let mut callees = resolved
            .iter()
            .filter(|edge| edge.source_symbol_id == source.id)
            .map(|edge| edge.target_symbol_id.clone())
            .collect::<Vec<_>>();
        callees.sort();
        callees
    };
    let ids = |records: &[&SymbolRecord]| {
        let mut ids = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };

    // `from kit import slug, resize` and `kit.slug()`: `kit/__init__.py` takes `slug` from
    // `text` with `*` and imports `scale` as `resize`.
    assert_eq!(callees(test_kit), ids(&[slug, scale]));
    // `from billing import total`, from `app/tests/`, reaches `app/src/billing/`.
    assert_eq!(callees(test_total), ids(&[total, amount]));
    Ok(())
}
