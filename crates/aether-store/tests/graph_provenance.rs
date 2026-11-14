//! Parser-to-store reproductions: an unrelated declaration with the right spelling is
//! not evidence that a Rust or ECMAScript reference reaches it.
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use aether_core::{EdgeKind, Symbol};
use aether_parse::{ExtractedFile, SymbolExtractor};
use aether_store::{
    GraphStore, ResolvedEdge, SqliteGraphStore, SqliteStore, StoreError, SymbolCatalogStore,
    SymbolRecord, SymbolRelationStore,
};
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn workspace() -> TestResult<TempDir> {
    let temp = tempfile::tempdir()?;
    write(
        temp.path(),
        ".aether/config.toml",
        "[storage]\ngraph_backend = \"sqlite\"\n[embeddings]\nenabled = false\n",
    )?;
    Ok(temp)
}

fn write(root: &Path, path: &str, source: &str) -> TestResult {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source)?;
    Ok(())
}

fn extract(extractor: &mut SymbolExtractor, path: &str, source: &str) -> TestResult<ExtractedFile> {
    Ok(extractor.extract_with_edges_from_path(Path::new(path), source)?)
}

/// Mirror the indexer's file replacement: remove retired identities and outgoing
/// edges, upsert this extraction, then refresh both neighbor directions.
fn index(store: &SqliteStore, path: &str, extracted: &ExtractedFile) -> TestResult {
    for old in store.list_symbols_for_file(path)? {
        if !extracted.symbols.iter().any(|symbol| symbol.id == old.id) {
            store.mark_removed(&old.id)?;
        }
    }
    store.delete_edges_for_file(path)?;
    for symbol in &extracted.symbols {
        store.upsert_symbol(record(symbol))?;
    }
    store.upsert_edges(&extracted.edges)?;
    store.populate_symbol_neighbors(path)?;
    Ok(())
}

fn record(symbol: &Symbol) -> SymbolRecord {
    SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.as_str().to_owned(),
        kind: symbol.kind.as_str().to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: 1,
    }
}

fn symbol<'a>(file: &'a ExtractedFile, name: &str) -> &'a Symbol {
    file.symbols
        .iter()
        .find(|symbol| symbol.qualified_name == name)
        .unwrap_or_else(|| panic!("missing {name}: {:?}", file.symbols))
}

fn calls(store: &SqliteStore, source: &Symbol) -> TestResult<BTreeSet<String>> {
    Ok(store
        .list_graph_dependency_edges()?
        .into_iter()
        .filter(|edge| edge.edge_kind == "calls" && edge.source_symbol_id == source.id)
        .map(|edge| edge.target_symbol_id)
        .collect())
}

fn neighbors(store: &SqliteStore, source: &Symbol) -> TestResult<BTreeSet<String>> {
    Ok(store
        .get_symbol_neighbors_by_type(&source.id, "calls")?
        .into_iter()
        .map(|neighbor| neighbor.neighbor_id)
        .collect())
}

#[tokio::test]
async fn rust_same_spelling_in_other_projects_never_becomes_a_local_call_or_type_ref() -> TestResult
{
    let temp = workspace()?;
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?;
    let source = r#"
struct Buf;
impl Buf { fn new() -> Self { Self } }
fn helper() {}
fn run(value: Buf) { helper(); Buf::new(); }
"#;
    let a = extract(&mut extractor, "alpha/src/lib.rs", source)?;
    let b = extract(&mut extractor, "beta/src/lib.rs", source)?;
    let orphan = extract(
        &mut extractor,
        "unrelated/src/lib.rs",
        "fn orphan() { helper(); Buf::new(); }",
    )?;
    for (path, file) in [
        ("alpha/src/lib.rs", &a),
        ("beta/src/lib.rs", &b),
        ("unrelated/src/lib.rs", &orphan),
    ] {
        index(&store, path, file)?;
    }
    let run = symbol(&a, "run");
    let expected = BTreeSet::from([
        symbol(&a, "helper").id.clone(),
        symbol(&a, "Buf::new").id.clone(),
    ]);
    assert_eq!(calls(&store, run)?, expected);
    assert_eq!(neighbors(&store, run)?, expected);
    assert!(calls(&store, symbol(&orphan, "orphan"))?.is_empty());
    assert!(!store.has_dependency_between_files("alpha/src/lib.rs", "beta/src/lib.rs")?);
    let types = store
        .list_graph_dependency_edges()?
        .into_iter()
        .filter(|edge| edge.source_symbol_id == run.id && edge.edge_kind == "type_ref")
        .map(|edge| edge.target_symbol_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(types, BTreeSet::from([symbol(&a, "Buf").id.clone()]));

    let raw = store.list_symbol_edges_for_source_and_kinds(
        &symbol(&orphan, "orphan").id,
        &[EdgeKind::Calls],
    )?;
    assert_eq!(raw.len(), 2, "unproved spellings remain recorded");
    assert!(
        raw.iter()
            .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
    );
    let coverage = store.call_edge_coverage_for_sources(&[symbol(&orphan, "orphan").id.clone()])?;
    assert_eq!(
        (coverage.call_edges, coverage.resolved, coverage.unresolved),
        (2, 0, 2)
    );
    let graph = SqliteGraphStore::open(temp.path())?;
    let chain = graph.get_call_chain(&run.id, 2).await?;
    assert_eq!(
        chain
            .into_iter()
            .flatten()
            .map(|target| target.id)
            .collect::<BTreeSet<_>>(),
        expected
    );
    let callers = GraphStore::get_callers(&graph, "helper").await?;
    assert_eq!(
        callers
            .into_iter()
            .map(|caller| caller.id)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([symbol(&a, "run").id.clone(), symbol(&b, "run").id.clone()])
    );
    Ok(())
}

#[test]
fn typescript_local_calls_use_lexical_proof_and_keep_unresolved_references() -> TestResult {
    let temp = workspace()?;
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?;
    let a = extract(
        &mut extractor,
        "alpha/main.ts",
        "function helper() {} function run() { helper(); } function shadow(helper: () => void) { helper(); }",
    )?;
    let b = extract(
        &mut extractor,
        "beta/main.ts",
        "function helper() {} function run() { helper(); }",
    )?;
    let orphan = extract(
        &mut extractor,
        "other/main.ts",
        "function orphan() { helper(); }",
    )?;
    for (path, file) in [
        ("alpha/main.ts", &a),
        ("beta/main.ts", &b),
        ("other/main.ts", &orphan),
    ] {
        index(&store, path, file)?;
    }
    assert_eq!(
        calls(&store, symbol(&a, "run"))?,
        BTreeSet::from([symbol(&a, "helper").id.clone()])
    );
    assert_eq!(
        neighbors(&store, symbol(&a, "run"))?,
        BTreeSet::from([symbol(&a, "helper").id.clone()])
    );
    assert!(calls(&store, symbol(&a, "shadow"))?.is_empty());
    assert!(calls(&store, symbol(&orphan, "orphan"))?.is_empty());
    let raw = store.list_symbol_edges_for_source_and_kinds(
        &symbol(&orphan, "orphan").id,
        &[EdgeKind::Calls],
    )?;
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].target_qualified_name, "helper");
    assert_eq!(raw[0].target_symbol_id, None);
    assert_eq!(raw[0].target_file_path, None);
    Ok(())
}

#[tokio::test]
async fn named_relative_import_aliases_cross_files_and_ecmascript_languages_only() -> TestResult {
    let temp = workspace()?;
    let main = "import { helper as imported } from './dep'; export function run() { imported(); }";
    let dep = "export function helper() {}";
    write(temp.path(), "alpha/main.js", main)?;
    write(temp.path(), "alpha/dep.ts", dep)?;
    write(temp.path(), "beta/dep.ts", dep)?;
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let a = extract(&mut extractor, "alpha/main.js", main)?;
    let target = extract(&mut extractor, "alpha/dep.ts", dep)?;
    let unrelated = extract(&mut extractor, "beta/dep.ts", dep)?;
    let rust = extract(&mut extractor, "elsewhere/dep.rs", "fn helper() {}")?;
    // Imported source is indexed before its proven target; a sole unrelated helper
    // must not satisfy it in that interval.
    index(&store, "beta/dep.ts", &unrelated)?;
    index(&store, "elsewhere/dep.rs", &rust)?;
    index(&store, "alpha/main.js", &a)?;
    assert!(calls(&store, symbol(&a, "run"))?.is_empty());
    index(&store, "alpha/dep.ts", &target)?;
    let expected = BTreeSet::from([symbol(&target, "helper").id.clone()]);
    assert_eq!(calls(&store, symbol(&a, "run"))?, expected);
    assert_eq!(neighbors(&store, symbol(&a, "run"))?, expected);
    assert!(store.has_dependency_between_files("alpha/main.js", "alpha/dep.ts")?);
    assert!(!store.has_dependency_between_files("alpha/main.js", "beta/dep.ts")?);
    assert!(!store.has_dependency_between_files("alpha/main.js", "elsewhere/dep.rs")?);
    let raw =
        store.list_symbol_edges_for_source_and_kinds(&symbol(&a, "run").id, &[EdgeKind::Calls])?;
    assert_eq!(
        raw,
        a.edges
            .iter()
            .filter(
                |edge| edge.source_id == symbol(&a, "run").id && edge.edge_kind == EdgeKind::Calls
            )
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(raw[0].target_file_path.as_deref(), Some("alpha/dep.ts"));
    let coverage = store.call_edge_coverage_for_sources(&[symbol(&a, "run").id.clone()])?;
    assert_eq!(
        (coverage.call_edges, coverage.resolved, coverage.unresolved),
        (1, 1, 0)
    );
    let graph = SqliteGraphStore::open(temp.path())?;
    assert_eq!(
        graph.get_call_chain(&symbol(&a, "run").id, 1).await?[0]
            .iter()
            .map(|target| target.id.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    Ok(())
}

#[test]
fn rust_explicit_import_alias_keeps_module_and_project_provenance() -> TestResult {
    let temp = workspace()?;
    let main = "mod dep; use crate::dep::helper as imported; fn run() { imported(); }";
    let dep = "pub fn helper() {}";
    for project in ["alpha", "beta"] {
        write(
            temp.path(),
            &format!("{project}/Cargo.toml"),
            &format!("[package]\nname = \"{project}\"\nversion = \"0.1.0\"\n"),
        )?;
        write(temp.path(), &format!("{project}/src/lib.rs"), main)?;
        write(temp.path(), &format!("{project}/src/dep.rs"), dep)?;
    }
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let a = extract(&mut extractor, "alpha/src/lib.rs", main)?;
    let target = extract(&mut extractor, "alpha/src/dep.rs", dep)?;
    let unrelated = extract(&mut extractor, "beta/src/dep.rs", dep)?;
    index(&store, "beta/src/dep.rs", &unrelated)?;
    index(&store, "alpha/src/lib.rs", &a)?;
    assert!(calls(&store, symbol(&a, "run"))?.is_empty());
    index(&store, "alpha/src/dep.rs", &target)?;
    let expected = BTreeSet::from([symbol(&target, "helper").id.clone()]);
    assert_eq!(calls(&store, symbol(&a, "run"))?, expected);
    assert_eq!(neighbors(&store, symbol(&a, "run"))?, expected);
    assert!(!store.has_dependency_between_files("alpha/src/lib.rs", "beta/src/dep.rs")?);
    Ok(())
}

#[test]
fn missing_and_ambiguous_import_files_never_use_a_same_name_fallback() -> TestResult {
    let temp = workspace()?;
    let main = "import { helper } from './dep'; function run() { helper(); }";
    let missing = "import { helper } from './missing'; function runMissing() { helper(); }";
    let dep = "export function helper() {}";
    for (path, source) in [
        ("app/main.ts", main),
        ("app/missing_caller.ts", missing),
        ("app/dep.ts", dep),
        ("app/dep.js", dep),
        ("elsewhere/dep.ts", dep),
    ] {
        write(temp.path(), path, source)?;
    }
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let mut files = Vec::new();
    for (path, source) in [
        ("app/main.ts", main),
        ("app/missing_caller.ts", missing),
        ("app/dep.ts", dep),
        ("app/dep.js", dep),
        ("elsewhere/dep.ts", dep),
    ] {
        let file = extract(&mut extractor, path, source)?;
        index(&store, path, &file)?;
        files.push(file);
    }
    for (file, name) in [(&files[0], "run"), (&files[1], "runMissing")] {
        let caller = symbol(file, name);
        assert!(calls(&store, caller)?.is_empty());
        assert!(neighbors(&store, caller)?.is_empty());
        let raw = store.list_symbol_edges_for_source_and_kinds(&caller.id, &[EdgeKind::Calls])?;
        assert_eq!(
            raw.len(),
            1,
            "unsupported imports keep the raw call reference"
        );
        assert_eq!(raw[0].target_symbol_id, None);
        assert_eq!(raw[0].target_file_path, None);
    }
    Ok(())
}

#[test]
fn source_replacement_target_identity_changes_and_removal_repair_incoming_neighbors() -> TestResult
{
    let temp = workspace()?;
    let main = "import { helper } from './dep'; function run() { helper(); }";
    let dep = "export function helper() {}";
    write(temp.path(), "app/main.ts", main)?;
    write(temp.path(), "app/dep.ts", dep)?;
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let a = extract(&mut extractor, "app/main.ts", main)?;
    let target = extract(&mut extractor, "app/dep.ts", dep)?;
    index(&store, "app/main.ts", &a)?;
    index(&store, "app/dep.ts", &target)?;
    let caller = symbol(&a, "run");
    let old_target = symbol(&target, "helper");
    assert_eq!(
        neighbors(&store, caller)?,
        BTreeSet::from([old_target.id.clone()])
    );

    let changed_dep = "export function helper(value: number) {}";
    write(temp.path(), "app/dep.ts", changed_dep)?;
    let updated_target = extract(&mut extractor, "app/dep.ts", changed_dep)?;
    let new_target = symbol(&updated_target, "helper");
    assert_ne!(new_target.id, old_target.id);
    index(&store, "app/dep.ts", &updated_target)?;
    assert_eq!(
        calls(&store, caller)?,
        BTreeSet::from([new_target.id.clone()])
    );
    assert_eq!(
        neighbors(&store, caller)?,
        BTreeSet::from([new_target.id.clone()])
    );
    assert!(store.get_symbol_neighbors(&old_target.id)?.is_empty());

    let changed_main = "import { helper } from './dep'; function run() { absent(); }";
    write(temp.path(), "app/main.ts", changed_main)?;
    let replacement = extract(&mut extractor, "app/main.ts", changed_main)?;
    index(&store, "app/main.ts", &replacement)?;
    assert!(calls(&store, symbol(&replacement, "run"))?.is_empty());
    assert!(neighbors(&store, symbol(&replacement, "run"))?.is_empty());
    write(temp.path(), "app/main.ts", main)?;
    let restored = extract(&mut extractor, "app/main.ts", main)?;
    index(&store, "app/main.ts", &restored)?;
    assert_eq!(
        calls(&store, caller)?,
        BTreeSet::from([new_target.id.clone()])
    );

    fs::remove_file(temp.path().join("app/dep.ts"))?;
    store.mark_removed(&new_target.id)?;
    store.delete_edges_for_file("app/dep.ts")?;
    assert!(calls(&store, caller)?.is_empty());
    assert!(neighbors(&store, caller)?.is_empty());
    // Removing a target never discards the import reference needed to reconnect it.
    assert_eq!(
        store
            .list_symbol_edges_for_source_and_kinds(&caller.id, &[EdgeKind::Calls])?
            .len(),
        1
    );
    let affected = extractor.refresh_workspace_file("app/dep.ts");
    assert!(
        affected.iter().any(|path| path == "app/main.ts"),
        "{affected:?}"
    );
    let missing_target = extract(&mut extractor, "app/main.ts", main)?;
    index(&store, "app/main.ts", &missing_target)?;
    assert!(calls(&store, symbol(&missing_target, "run"))?.is_empty());
    Ok(())
}

/// Records the file-level writes sent to the external graph backend. This verifies
/// projection invalidation independently of SQLite's on-demand call-chain queries.
#[derive(Default)]
struct RecordingGraph {
    edges: Mutex<Vec<ResolvedEdge>>,
}

impl RecordingGraph {
    fn edges(&self) -> Vec<ResolvedEdge> {
        self.edges.lock().unwrap().clone()
    }

    fn call_pairs(&self, file_path: &str) -> BTreeSet<(String, String)> {
        self.edges()
            .into_iter()
            .filter(|edge| edge.file_path == file_path && edge.edge_kind == EdgeKind::Calls)
            .map(|edge| (edge.source_id, edge.target_id))
            .collect()
    }
}

#[async_trait::async_trait]
impl GraphStore for RecordingGraph {
    async fn upsert_symbol_node(&self, _symbol: &SymbolRecord) -> Result<(), StoreError> {
        Ok(())
    }
    async fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        self.edges.lock().unwrap().push(edge.clone());
        Ok(())
    }
    async fn get_callers(&self, _qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        Ok(Vec::new())
    }
    async fn get_dependencies(&self, _symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        Ok(Vec::new())
    }
    async fn get_call_chain(
        &self,
        _symbol_id: &str,
        _depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        Ok(Vec::new())
    }
    async fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        self.edges
            .lock()
            .unwrap()
            .retain(|edge| edge.file_path != file_path);
        Ok(())
    }
    async fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        self.edges.lock().unwrap().retain(|edge| {
            !symbol_ids.contains(&edge.source_id) && !symbol_ids.contains(&edge.target_id)
        });
        Ok(())
    }
}

#[tokio::test]
async fn projected_imports_refresh_when_the_target_id_changes_or_all_declarations_disappear()
-> TestResult {
    let temp = workspace()?;
    let main = "import { helper } from './dep'; function run() { helper(); }";
    let dep = "export function helper() {}";
    write(temp.path(), "app/main.ts", main)?;
    write(temp.path(), "app/dep.ts", dep)?;
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let a = extract(&mut extractor, "app/main.ts", main)?;
    let target = extract(&mut extractor, "app/dep.ts", dep)?;
    index(&store, "app/main.ts", &a)?;
    index(&store, "app/dep.ts", &target)?;
    let graph = RecordingGraph::default();
    store.sync_graph_for_file(&graph, "app/dep.ts").await?;
    assert_eq!(
        graph
            .edges()
            .iter()
            .map(|edge| edge.target_id.clone())
            .collect::<Vec<_>>(),
        [symbol(&target, "helper").id.clone()]
    );

    let updated_source = "export function helper(value: number) {}";
    write(temp.path(), "app/dep.ts", updated_source)?;
    let updated = extract(&mut extractor, "app/dep.ts", updated_source)?;
    index(&store, "app/dep.ts", &updated)?;
    store.sync_graph_for_file(&graph, "app/dep.ts").await?;
    assert_eq!(
        graph
            .edges()
            .iter()
            .map(|edge| edge.target_id.clone())
            .collect::<Vec<_>>(),
        [symbol(&updated, "helper").id.clone()]
    );

    fs::remove_file(temp.path().join("app/dep.ts"))?;
    store.mark_removed(&symbol(&updated, "helper").id)?;
    store.delete_edges_for_file("app/dep.ts")?;
    store.sync_graph_for_file(&graph, "app/dep.ts").await?;
    assert!(graph.edges().is_empty(), "{:?}", graph.edges());
    Ok(())
}

#[tokio::test]
async fn imported_rust_nominal_receivers_keep_project_identity_through_target_changes() -> TestResult
{
    let temp = workspace()?;
    let main = r#"
mod dep;
use crate::dep::Buf as Input;
fn run(buf: Input) { buf.len(); }
fn make() { let buf = Input; buf.len(); }
fn unknown<T>(buf: T) { buf.len(); }
"#;
    let dep = "pub struct Buf; impl Buf { pub fn len(&self) -> usize { 1 } }";
    for project in ["alpha", "beta"] {
        write(
            temp.path(),
            &format!("{project}/Cargo.toml"),
            &format!("[package]\nname = \"{project}\"\nversion = \"0.1.0\"\n"),
        )?;
        write(temp.path(), &format!("{project}/src/lib.rs"), main)?;
        write(temp.path(), &format!("{project}/src/dep.rs"), dep)?;
    }
    let store = SqliteStore::open(temp.path())?;
    let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
    let a = extract(&mut extractor, "alpha/src/lib.rs", main)?;
    let b = extract(&mut extractor, "beta/src/lib.rs", main)?;
    let target = extract(&mut extractor, "alpha/src/dep.rs", dep)?;
    let other_target = extract(&mut extractor, "beta/src/dep.rs", dep)?;
    index(&store, "beta/src/dep.rs", &other_target)?;
    index(&store, "beta/src/lib.rs", &b)?;
    index(&store, "alpha/src/lib.rs", &a)?;
    for name in ["run", "make"] {
        assert!(
            calls(&store, symbol(&a, name))?.is_empty(),
            "the only indexed Buf::len is in another project"
        );
    }
    index(&store, "alpha/src/dep.rs", &target)?;
    let target_method = symbol(&target, "Buf::len");
    for name in ["run", "make"] {
        let caller = symbol(&a, name);
        let expected = BTreeSet::from([target_method.id.clone()]);
        assert_eq!(calls(&store, caller)?, expected);
        assert_eq!(neighbors(&store, caller)?, expected);
        let raw = store.list_symbol_edges_for_source_and_kinds(&caller.id, &[EdgeKind::Calls])?;
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].target_qualified_name, "Buf::len");
        assert_eq!(raw[0].target_file_path.as_deref(), Some("alpha/src/dep.rs"));
    }
    assert_eq!(
        calls(&store, symbol(&b, "run"))?,
        BTreeSet::from([symbol(&other_target, "Buf::len").id.clone()])
    );
    assert!(!store.has_dependency_between_files("alpha/src/lib.rs", "beta/src/dep.rs")?);
    let unknown = store
        .list_symbol_edges_for_source_and_kinds(&symbol(&a, "unknown").id, &[EdgeKind::Calls])?;
    assert_eq!(
        unknown.len(),
        1,
        "an unproved receiver still contributes raw evidence"
    );
    assert_eq!(unknown[0].target_qualified_name, "len");
    assert!(unknown[0].target_symbol_id.is_none() && unknown[0].target_file_path.is_none());
    assert!(calls(&store, symbol(&a, "unknown"))?.is_empty());
    let graph = RecordingGraph::default();
    store
        .sync_graph_for_file(&graph, "alpha/src/dep.rs")
        .await?;
    assert_eq!(
        graph.call_pairs("alpha/src/lib.rs"),
        BTreeSet::from([
            (symbol(&a, "run").id.clone(), target_method.id.clone()),
            (symbol(&a, "make").id.clone(), target_method.id.clone()),
        ])
    );

    // A return-type edit changes the target ID without changing its module or
    // making these calls invalid. Existing imported file evidence reconnects it.
    let changed_dep = "pub struct Buf; impl Buf { pub fn len(&self) -> u64 { 1 } }";
    write(temp.path(), "alpha/src/dep.rs", changed_dep)?;
    let affected = extractor.refresh_workspace_file("alpha/src/dep.rs");
    assert!(
        affected.iter().any(|path| path == "alpha/src/lib.rs"),
        "{affected:?}"
    );
    assert!(
        !affected.iter().any(|path| path == "beta/src/lib.rs"),
        "{affected:?}"
    );
    let changed_target = extract(&mut extractor, "alpha/src/dep.rs", changed_dep)?;
    let changed_method = symbol(&changed_target, "Buf::len");
    assert_ne!(changed_method.id, target_method.id);
    index(&store, "alpha/src/dep.rs", &changed_target)?;
    store
        .sync_graph_for_file(&graph, "alpha/src/dep.rs")
        .await?;
    for name in ["run", "make"] {
        assert_eq!(
            calls(&store, symbol(&a, name))?,
            BTreeSet::from([changed_method.id.clone()])
        );
        assert_eq!(
            neighbors(&store, symbol(&a, name))?,
            BTreeSet::from([changed_method.id.clone()])
        );
    }
    assert!(store.get_symbol_neighbors(&target_method.id)?.is_empty());
    assert_eq!(
        graph.call_pairs("alpha/src/lib.rs"),
        BTreeSet::from([
            (symbol(&a, "run").id.clone(), changed_method.id.clone()),
            (symbol(&a, "make").id.clone(), changed_method.id.clone()),
        ])
    );

    // Removing this module cannot redirect either receiver to beta's still
    // indexed, identically named method. Referring-file invalidation survives
    // the disappearance of every declaration in the removed module.
    fs::remove_file(temp.path().join("alpha/src/dep.rs"))?;
    let empty = extract(&mut extractor, "alpha/src/dep.rs", "")?;
    index(&store, "alpha/src/dep.rs", &empty)?;
    store
        .sync_graph_for_file(&graph, "alpha/src/dep.rs")
        .await?;
    assert!(
        !graph
            .edges()
            .iter()
            .any(|edge| edge.file_path == "alpha/src/lib.rs"),
        "{:?}",
        graph.edges()
    );
    for name in ["run", "make"] {
        assert!(calls(&store, symbol(&a, name))?.is_empty());
        assert!(neighbors(&store, symbol(&a, name))?.is_empty());
    }
    let affected = extractor.refresh_workspace_file("alpha/src/dep.rs");
    assert!(
        affected.iter().any(|path| path == "alpha/src/lib.rs"),
        "{affected:?}"
    );
    let reextracted = extract(&mut extractor, "alpha/src/lib.rs", main)?;
    index(&store, "alpha/src/lib.rs", &reextracted)?;
    for name in ["run", "make"] {
        let caller = symbol(&reextracted, name);
        let raw = store.list_symbol_edges_for_source_and_kinds(&caller.id, &[EdgeKind::Calls])?;
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].target_qualified_name, "len");
        assert!(raw[0].target_symbol_id.is_none() && raw[0].target_file_path.is_none());
        assert!(calls(&store, caller)?.is_empty());
    }
    assert_eq!(
        calls(&store, symbol(&b, "run"))?,
        BTreeSet::from([symbol(&other_target, "Buf::len").id.clone()])
    );
    Ok(())
}

#[tokio::test]
async fn eval_mutations_invalidate_local_and_imported_proofs() -> TestResult {
    for extension in ["ts", "mjs"] {
        let temp = workspace()?;
        let caller_path = format!("alpha/main.{extension}");
        let dep_path = format!("alpha/dep.{extension}");
        let other_caller_path = format!("beta/main.{extension}");
        let other_dep_path = format!("beta/dep.{extension}");
        let local_path = format!("alpha/local.{extension}");
        let caller_source = format!(
            "import {{ helper }} from './dep.{extension}'; export function run() {{ return helper(); }}"
        );
        let clean_dep = "export function helper() { return 1; }";
        let mutated_dep = r#"export function helper() { return 1; } eval("helper = () => 2");"#;
        let local_source = r#"function helper() { return 1; } export function run() { eval("helper = () => 3"); return helper(); }"#;
        for (path, source) in [
            (&caller_path, caller_source.as_str()),
            (&dep_path, clean_dep),
            (&other_caller_path, caller_source.as_str()),
            (&other_dep_path, clean_dep),
            (&local_path, local_source),
        ] {
            write(temp.path(), path, source)?;
        }
        let store = SqliteStore::open(temp.path())?;
        let mut extractor = SymbolExtractor::new()?.with_workspace_root(temp.path());
        let caller = extract(&mut extractor, &caller_path, &caller_source)?;
        let dep = extract(&mut extractor, &dep_path, clean_dep)?;
        let other_caller = extract(&mut extractor, &other_caller_path, &caller_source)?;
        let other_dep = extract(&mut extractor, &other_dep_path, clean_dep)?;
        let local = extract(&mut extractor, &local_path, local_source)?;
        for (path, extracted) in [
            (&caller_path, &caller),
            (&dep_path, &dep),
            (&other_caller_path, &other_caller),
            (&other_dep_path, &other_dep),
            (&local_path, &local),
        ] {
            index(&store, path, extracted)?;
        }
        let run = symbol(&caller, "run");
        let helper = symbol(&dep, "helper");
        let other_run = symbol(&other_caller, "run");
        let other_helper = symbol(&other_dep, "helper");
        let expected = BTreeSet::from([helper.id.clone()]);
        let other_expected = BTreeSet::from([other_helper.id.clone()]);
        assert_eq!(calls(&store, run)?, expected);
        assert_eq!(calls(&store, other_run)?, other_expected);
        let graph = RecordingGraph::default();
        store.sync_graph_for_file(&graph, &caller_path).await?;
        store
            .sync_graph_for_file(&graph, &other_caller_path)
            .await?;
        assert_eq!(
            graph.call_pairs(&caller_path),
            BTreeSet::from([(run.id.clone(), helper.id.clone())])
        );
        let local_run = symbol(&local, "run");
        let raw_local =
            store.list_symbol_edges_for_source_and_kinds(&local_run.id, &[EdgeKind::Calls])?;
        assert!(
            raw_local
                .iter()
                .any(|edge| edge.target_qualified_name == "helper")
        );
        assert!(
            raw_local
                .iter()
                .all(|edge| edge.target_symbol_id.is_none() && edge.target_file_path.is_none())
        );
        assert!(calls(&store, local_run)?.is_empty());
        assert!(neighbors(&store, local_run)?.is_empty());

        // Adding eval outside the declaration leaves its identity unchanged,
        // but invalidates the import's proof of that declaration as the callee.
        write(temp.path(), &dep_path, mutated_dep)?;
        let affected = extractor.refresh_workspace_file(&dep_path);
        assert!(affected.contains(&caller_path), "{affected:?}");
        assert!(!affected.contains(&other_caller_path), "{affected:?}");
        let changed_dep = extract(&mut extractor, &dep_path, mutated_dep)?;
        assert_eq!(symbol(&changed_dep, "helper").id, helper.id);
        index(&store, &dep_path, &changed_dep)?;
        let invalidated = extract(&mut extractor, &caller_path, &caller_source)?;
        assert_eq!(symbol(&invalidated, "run").id, run.id);
        index(&store, &caller_path, &invalidated)?;
        store.sync_graph_for_file(&graph, &caller_path).await?;
        let raw = store.list_symbol_edges_for_source_and_kinds(&run.id, &[EdgeKind::Calls])?;
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].target_qualified_name, "helper");
        assert!(raw[0].target_symbol_id.is_none() && raw[0].target_file_path.is_none());
        assert!(calls(&store, run)?.is_empty());
        assert!(neighbors(&store, run)?.is_empty());
        assert!(store.list_callers_for_symbol(&helper.id)?.is_empty());
        assert!(graph.call_pairs(&caller_path).is_empty());
        assert_eq!(calls(&store, other_run)?, other_expected);
        assert_eq!(
            graph.call_pairs(&other_caller_path),
            BTreeSet::from([(other_run.id.clone(), other_helper.id.clone())])
        );

        // An unresolved import still tracks its candidate, so removing eval
        // restores the legitimate link without changing either symbol's ID.
        write(temp.path(), &dep_path, clean_dep)?;
        assert!(
            extractor
                .refresh_workspace_file(&dep_path)
                .contains(&caller_path)
        );
        index(&store, &dep_path, &dep)?;
        let restored = extract(&mut extractor, &caller_path, &caller_source)?;
        index(&store, &caller_path, &restored)?;
        store.sync_graph_for_file(&graph, &caller_path).await?;
        assert_eq!(calls(&store, run)?, expected);
        assert_eq!(neighbors(&store, run)?, expected);
        assert_eq!(
            graph.call_pairs(&caller_path),
            BTreeSet::from([(run.id.clone(), helper.id.clone())])
        );
    }
    Ok(())
}
