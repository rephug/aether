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
        workspace.join("go.mod"),
        "module example.com/shop\n\ngo 1.22\n",
    )?;
    fs::create_dir_all(workspace.join("store"))?;
    fs::write(
        workspace.join("store/store.go"),
        r#"package store

import (
	"errors"
	"fmt"

	"example.com/shop/internal/db"
)

// Store keeps items by key.
type Store struct {
	items map[string]string
}

func NewStore() *Store {
	return &Store{items: map[string]string{}}
}

func (s *Store) Get(key string) (string, error) {
	value, ok := s.lookup(key)
	if !ok {
		return "", fmt.Errorf("missing %s", key)
	}
	return value, nil
}

func (s *Store) Save(conn *db.Conn) error {
	conn.Ping()
	db.Open()
	return errors.New("saved")
}
"#,
    )?;
    fs::create_dir_all(workspace.join("internal/db"))?;
    fs::write(
        workspace.join("internal/db/db.go"),
        r#"package db

type Conn struct{}

func Open() *Conn { return &Conn{} }

func (c *Conn) Ping() error { return nil }
"#,
    )?;
    // A workspace package that shares its name with a standard-library package.
    fs::create_dir_all(workspace.join("internal/errors"))?;
    fs::write(
        workspace.join("internal/errors/errors.go"),
        r#"package errors

func New(message string) error { return nil }
"#,
    )?;
    fs::write(
        workspace.join("store/lookup.go"),
        r#"package store

func (s *Store) lookup(key string) (string, bool) {
	value, ok := s.items[key]
	return value, ok
}
"#,
    )?;
    fs::write(
        workspace.join("store/store_test.go"),
        r#"package store

import "testing"

// TestGetReturnsStoredValue checks that a stored value comes back.
func TestGetReturnsStoredValue(t *testing.T) {
	s := NewStore()
	s.items["a"] = "b"
	if got, _ := s.Get("a"); got != "b" {
		t.Fatalf("got %q", got)
	}
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
fn structural_index_stores_go_symbols_edges_and_test_intents()
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
        "store/store.go",
        "store/lookup.go",
        "store/store_test.go",
        "internal/db/db.go",
        "internal/errors/errors.go",
    ] {
        symbols.extend(store.list_symbols_for_file(file)?);
    }
    assert!(symbols.iter().all(|symbol| symbol.language == "go"));

    let store_type = find(&symbols, "store::Store");
    let new_store = find(&symbols, "store::NewStore");
    let get = find(&symbols, "store::Store::Get");
    let save = find(&symbols, "store::Store::Save");
    let lookup = find(&symbols, "store::Store::lookup");
    let test = find(&symbols, "store::TestGetReturnsStoredValue");
    let conn = find(&symbols, "db::Conn");
    let ping = find(&symbols, "db::Conn::Ping");
    let open = find(&symbols, "db::Open");
    let workspace_new = find(&symbols, "errors::New");
    assert_eq!(store_type.kind, "struct");
    assert_eq!(new_store.kind, "function");
    assert_eq!(get.kind, "method");
    assert_eq!(lookup.file_path, "store/lookup.go");

    // Edges resolve across the files of one package and into another package of the module:
    // a method calls a method declared in another file, a constructor references the type it
    // returns, and a method calls another package's function and a parameter's method.
    let resolved = store
        .list_graph_dependency_edges()?
        .into_iter()
        .map(|edge| (edge.source_symbol_id, edge.target_symbol_id, edge.edge_kind))
        .collect::<Vec<_>>();
    let has_edge = |source: &str, target: &str, kind: &str| {
        resolved
            .iter()
            .any(|(edge_source, edge_target, edge_kind)| {
                edge_source == source && edge_target == target && edge_kind == kind
            })
    };
    for (source, target, kind) in [
        (&get.id, &lookup.id, "calls"),
        (&new_store.id, &store_type.id, "type_ref"),
        (&test.id, &new_store.id, "calls"),
        (&save.id, &ping.id, "calls"),
        (&save.id, &open.id, "calls"),
        (&save.id, &conn.id, "type_ref"),
    ] {
        assert!(
            has_edge(source, target, kind),
            "missing resolved edge {source} -> {target} ({kind}) in {resolved:?}"
        );
    }
    // `errors.New` here is the standard library's, so it must not resolve to the workspace
    // package that is also called `errors`.
    assert!(
        !has_edge(&save.id, &workspace_new.id, "calls"),
        "the standard library's errors.New resolved to the workspace errors::New"
    );

    let intents = store.list_test_intents_for_file("store/store_test.go")?;
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].test_name, "TestGetReturnsStoredValue");
    assert_eq!(
        intents[0].intent_text,
        "checks that a stored value comes back."
    );
    assert_eq!(intents[0].language, "go");
    assert_eq!(intents[0].symbol_id.as_deref(), Some(test.id.as_str()));

    Ok(())
}
