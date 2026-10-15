//! End-to-end `--index-once` runs against a store indexed before the workspace changed:
//! edited symbols, symbols removed or re-identified, and the file rollups over them.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use aether_config::InferenceProviderKind;
use aether_core::Symbol;
use aether_parse::SymbolExtractor;
use aether_sir::{FileSir, synthetic_file_sir_id};
use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore};
use tempfile::tempdir;

use super::{IndexerConfig, run_initial_index_once};

const MODULE: &str = "pkg/mod.py";
const GONE: &str = "pkg/gone.py";

const MODULE_V1: &str = "def keep(value):
    return value + 1


def edited(value):
    return value > 1


def removed(value):
    return value - 1
";

/// `edited` gets a new body (and one more line); its signature, and so its id, stay.
const MODULE_BODY_EDIT: &str = "def keep(value):
    return value + 1


def edited(value):
    threshold = 2
    return value > threshold


def removed(value):
    return value - 1
";

/// `edited` gets a new signature, and so a new id; `removed` is deleted.
const MODULE_REIDENTIFIED: &str = "def keep(value):
    return value + 1


def edited(value, limit=2):
    return value > limit
";

/// `removed` is deleted; nothing else changes.
const MODULE_WITHOUT_REMOVED: &str = "def keep(value):
    return value + 1


def edited(value):
    return value > 1
";

const GONE_V1: &str = "def vanish():
    return None
";

fn write_workspace(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "mock"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
vector_backend = "sqlite"
"#,
    )
    .expect("write config");
    fs::create_dir_all(workspace.join("pkg")).expect("create pkg");
    fs::write(workspace.join(MODULE), MODULE_V1).expect("write module");
    fs::write(workspace.join(GONE), GONE_V1).expect("write second module");
}

fn index_once(workspace: &Path, full: bool) {
    run_initial_index_once(&IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        embeddings_only: false,
        force: false,
        full,
        deep: false,
        turbo_concurrency: None,
        dry_run: false,
        sir_concurrency: 2,
        lifecycle_logs: false,
        inference_provider: Some(InferenceProviderKind::Mock),
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        pause_flag: None,
    })
    .expect("index once");
}

/// The symbols the parser finds in `file_path` as it is on disk now.
fn parse(workspace: &Path, file_path: &str) -> Vec<Symbol> {
    let source = fs::read_to_string(workspace.join(file_path)).expect("read source");
    SymbolExtractor::new()
        .expect("parser")
        .extract_from_path(Path::new(file_path), &source)
        .expect("parse source")
}

fn named(symbols: &[Symbol], name: &str) -> Symbol {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol named {name}"))
        .clone()
}

struct Leaf {
    status: String,
    version: i64,
    source_hash: Option<String>,
}

fn leaf(store: &SqliteStore, symbol_id: &str) -> Option<Leaf> {
    let meta = store.get_sir_meta(symbol_id).expect("read SIR meta")?;
    Some(Leaf {
        status: meta.sir_status,
        version: meta.sir_version,
        source_hash: store.get_sir_source_hash(symbol_id).expect("read source"),
    })
}

fn stored_symbol_ids(store: &SqliteStore) -> BTreeSet<String> {
    store
        .list_all_symbol_ids()
        .expect("list symbols")
        .into_iter()
        .collect()
}

fn rollup_id(file_path: &str) -> String {
    synthetic_file_sir_id("python", file_path)
}

fn rollup(store: &SqliteStore, file_path: &str) -> Option<FileSir> {
    store
        .read_sir_blob(&rollup_id(file_path))
        .expect("read rollup")
        .map(|blob| serde_json::from_str(&blob).expect("rollup JSON"))
}

fn mirror_path(workspace: &Path, symbol_id: &str) -> std::path::PathBuf {
    workspace
        .join(".aether/sir")
        .join(format!("{symbol_id}.json"))
}

#[test]
fn full_index_once_regenerates_a_symbol_whose_body_changed() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);

    let before = named(&parse(workspace, MODULE), "edited");
    let keep_before = named(&parse(workspace, MODULE), "keep");
    let store = SqliteStore::open(workspace).expect("open store");
    let first = leaf(&store, &before.id).expect("first SIR");
    assert_eq!(first.status, "fresh");
    assert_eq!(
        first.source_hash.as_deref(),
        Some(before.content_hash.as_str())
    );
    let keep_first = leaf(&store, &keep_before.id).expect("keep SIR");
    drop(store);

    fs::write(workspace.join(MODULE), MODULE_BODY_EDIT).expect("edit body");
    let after = named(&parse(workspace, MODULE), "edited");
    assert_eq!(after.id, before.id, "a body edit keeps the symbol id");
    assert_ne!(after.content_hash, before.content_hash);

    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("open store");
    let regenerated = leaf(&store, &after.id).expect("regenerated SIR");
    assert_eq!(regenerated.status, "fresh");
    assert_eq!(
        regenerated.source_hash.as_deref(),
        Some(after.content_hash.as_str()),
        "the SIR must describe the edited body"
    );
    assert!(regenerated.version > first.version);
    let keep_again = leaf(&store, &keep_before.id).expect("keep SIR");
    assert_eq!(
        keep_again.version, keep_first.version,
        "an unchanged symbol is not regenerated"
    );
}

#[test]
fn structural_index_once_marks_a_symbol_whose_body_changed_stale() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let keep = named(&parse(workspace, MODULE), "keep");

    fs::write(workspace.join(MODULE), MODULE_BODY_EDIT).expect("edit body");
    let edited = named(&parse(workspace, MODULE), "edited");
    index_once(workspace, false);

    let store = SqliteStore::open(workspace).expect("open store");
    assert_eq!(leaf(&store, &edited.id).expect("SIR").status, "stale");
    assert_eq!(leaf(&store, &keep.id).expect("SIR").status, "fresh");
    drop(store);

    // The stale leaf is what the next full run regenerates.
    index_once(workspace, true);
    let store = SqliteStore::open(workspace).expect("open store");
    let regenerated = leaf(&store, &edited.id).expect("SIR");
    assert_eq!(regenerated.status, "fresh");
    assert_eq!(
        regenerated.source_hash.as_deref(),
        Some(edited.content_hash.as_str())
    );
}

#[test]
fn full_index_once_regenerates_a_symbol_whose_id_changed() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let old = named(&parse(workspace, MODULE), "edited");

    fs::write(workspace.join(MODULE), MODULE_REIDENTIFIED).expect("change signature");
    let new = named(&parse(workspace, MODULE), "edited");
    assert_ne!(
        new.id, old.id,
        "a signature change re-identifies the symbol"
    );

    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("open store");
    assert!(leaf(&store, &old.id).is_none());
    let migrated = leaf(&store, &new.id).expect("SIR under the new id");
    assert_eq!(migrated.status, "fresh");
    assert_eq!(
        migrated.source_hash.as_deref(),
        Some(new.content_hash.as_str()),
        "the SIR carried over from the old id must be regenerated for the new body"
    );
}

#[test]
fn structural_index_once_prunes_removed_and_reidentified_symbols() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let v1 = parse(workspace, MODULE);
    let removed = named(&v1, "removed");
    let old_edited = named(&v1, "edited");
    let vanished = named(&parse(workspace, GONE), "vanish");
    assert!(mirror_path(workspace, &removed.id).exists());

    fs::write(workspace.join(MODULE), MODULE_REIDENTIFIED).expect("rewrite module");
    fs::remove_file(workspace.join(GONE)).expect("delete second module");
    let live = parse(workspace, MODULE);
    let new_edited = named(&live, "edited");

    index_once(workspace, false);

    let store = SqliteStore::open(workspace).expect("open store");
    let expected = live
        .iter()
        .map(|symbol| symbol.id.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(stored_symbol_ids(&store), expected);
    for gone in [&removed.id, &old_edited.id, &vanished.id] {
        assert!(leaf(&store, gone).is_none(), "SIR of {gone} must be pruned");
        assert!(
            !mirror_path(workspace, gone).exists(),
            "mirror of {gone} must be removed with its row"
        );
    }
    // The re-identified symbol keeps its SIR under the new id, flagged as describing
    // the old body.
    assert_eq!(
        leaf(&store, &new_edited.id).expect("migrated SIR").status,
        "stale"
    );
}

#[test]
fn full_index_once_rebuilds_the_rollup_of_a_file_that_lost_a_symbol() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    {
        let store = SqliteStore::open(workspace).expect("open store");
        let rollup = rollup(&store, MODULE).expect("rollup");
        assert_eq!(rollup.symbol_count, 3);
    }

    fs::write(workspace.join(MODULE), MODULE_WITHOUT_REMOVED).expect("remove a function");
    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("open store");
    let rollup = rollup(&store, MODULE).expect("rollup");
    assert_eq!(rollup.symbol_count, 2);
    assert!(
        rollup.exports.iter().all(|name| !name.contains("removed")),
        "the rollup still lists a deleted symbol: {:?}",
        rollup.exports
    );
    assert_eq!(
        store
            .get_sir_meta(&rollup_id(MODULE))
            .expect("meta")
            .expect("rollup meta")
            .sir_status,
        "fresh"
    );
}

#[test]
fn structural_index_once_flags_the_rollups_of_changed_files() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);

    fs::write(workspace.join(MODULE), MODULE_WITHOUT_REMOVED).expect("remove a function");
    fs::remove_file(workspace.join(GONE)).expect("delete second module");
    index_once(workspace, false);

    let store = SqliteStore::open(workspace).expect("open store");
    assert_eq!(
        store
            .get_sir_meta(&rollup_id(MODULE))
            .expect("meta")
            .expect("rollup meta")
            .sir_status,
        "stale"
    );
    assert!(
        store
            .get_sir_meta(&rollup_id(GONE))
            .expect("meta")
            .is_none(),
        "the rollup of a deleted file must be removed"
    );
    drop(store);

    // The next full run rebuilds the flagged rollup.
    index_once(workspace, true);
    let store = SqliteStore::open(workspace).expect("open store");
    assert_eq!(rollup(&store, MODULE).expect("rollup").symbol_count, 2);
    assert_eq!(
        store
            .get_sir_meta(&rollup_id(MODULE))
            .expect("meta")
            .expect("rollup meta")
            .sir_status,
        "fresh"
    );
}

#[test]
fn full_index_once_rebuilds_a_missing_rollup() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    {
        let store = SqliteStore::open(workspace).expect("open store");
        assert!(rollup(&store, MODULE).is_some());
        store
            .mark_removed(&rollup_id(MODULE))
            .expect("delete rollup");
        assert!(rollup(&store, MODULE).is_none());
    }

    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("open store");
    assert_eq!(
        rollup(&store, MODULE).expect("rebuilt rollup").symbol_count,
        3
    );
}
