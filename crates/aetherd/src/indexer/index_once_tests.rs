//! End-to-end `--index-once` runs against a store indexed before the workspace changed:
//! edited symbols, symbols removed or re-identified, and the file rollups over them.
//! The workspace and store helpers are shared with `startup_tests`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use aether_config::InferenceProviderKind;
use aether_core::{Language, Symbol, SymbolChangeEvent};
use aether_parse::SymbolExtractor;
use aether_sir::{FileSir, synthetic_file_sir_id};
use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore};
use tempfile::tempdir;

use super::reconciliation::SnapshotScope;
use super::snapshot::sync_snapshot;
use super::structural::StructuralIndexer;
use super::{IndexerConfig, ObserverState, collect_initial_snapshot, run_initial_index_once};

pub(super) const MODULE: &str = "pkg/mod.py";
pub(super) const GONE: &str = "pkg/gone.py";

pub(super) const MODULE_V1: &str = "def keep(value):
    return value + 1


def edited(value):
    return value > 1


def removed(value):
    return value - 1
";

/// `edited` gets a new body (and one more line); its signature, and so its id, stay.
pub(super) const MODULE_BODY_EDIT: &str = "def keep(value):
    return value + 1


def edited(value):
    threshold = 2
    return value > threshold


def removed(value):
    return value - 1
";

/// `edited` gets a new signature, and so a new id; `removed` is deleted.
pub(super) const MODULE_REIDENTIFIED: &str = "def keep(value):
    return value + 1


def edited(value, limit=2):
    return value > limit
";

/// `removed` is deleted; nothing else changes.
pub(super) const MODULE_WITHOUT_REMOVED: &str = "def keep(value):
    return value + 1


def edited(value):
    return value > 1
";

pub(super) const GONE_V1: &str = "def vanish():
    return None
";

/// What a daemon indexes while a run's older snapshot is in flight: `keep` gets a new
/// body, `edited` a new signature, and `removed` is deleted.
const MODULE_EDITED_BY_DAEMON: &str = "def keep(value):
    return value + 2


def edited(value, limit=2):
    return value > limit
";

pub(super) fn write_workspace(workspace: &Path) {
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

pub(super) fn index_once(workspace: &Path, full: bool) {
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
pub(super) fn parse(workspace: &Path, file_path: &str) -> Vec<Symbol> {
    let source = fs::read_to_string(workspace.join(file_path)).expect("read source");
    SymbolExtractor::new()
        .expect("parser")
        .extract_from_path(Path::new(file_path), &source)
        .expect("parse source")
}

pub(super) fn named(symbols: &[Symbol], name: &str) -> Symbol {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol named {name}"))
        .clone()
}

pub(super) struct Leaf {
    pub(super) status: String,
    pub(super) version: i64,
    pub(super) source_hash: Option<String>,
}

pub(super) fn leaf(store: &SqliteStore, symbol_id: &str) -> Option<Leaf> {
    let meta = store.get_sir_meta(symbol_id).expect("read SIR meta")?;
    Some(Leaf {
        status: meta.sir_status,
        version: meta.sir_version,
        source_hash: store.get_sir_source_hash(symbol_id).expect("read source"),
    })
}

pub(super) fn stored_symbol_ids(store: &SqliteStore) -> BTreeSet<String> {
    store
        .list_all_symbol_ids()
        .expect("list symbols")
        .into_iter()
        .collect()
}

pub(super) fn rollup_id(file_path: &str) -> String {
    synthetic_file_sir_id("python", file_path)
}

pub(super) fn rollup(store: &SqliteStore, file_path: &str) -> Option<FileSir> {
    store
        .read_sir_blob(&rollup_id(file_path))
        .expect("read rollup")
        .map(|blob| serde_json::from_str(&blob).expect("rollup JSON"))
}

pub(super) fn mirror_path(workspace: &Path, symbol_id: &str) -> std::path::PathBuf {
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

fn rollup_status(store: &SqliteStore, file_path: &str) -> Option<String> {
    store
        .get_sir_meta(&rollup_id(file_path))
        .expect("read rollup meta")
        .map(|meta| meta.sir_status)
}

#[test]
fn rollups_of_pruned_files_stay_invalidated_when_the_structural_pass_fails() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let removed = named(&parse(workspace, MODULE), "removed");

    fs::write(workspace.join(MODULE), MODULE_WITHOUT_REMOVED).expect("remove a function");
    fs::remove_file(workspace.join(GONE)).expect("delete second module");
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let (mut events, symbols_by_id, _) = collect_initial_snapshot(&observer);
    // The structural pass fails after the prune has committed (as a cancelled run
    // stops): `pkg` is a directory, which the pass cannot read as source.
    events.push(SymbolChangeEvent {
        file_path: "pkg".to_owned(),
        language: Language::Python,
        added: Vec::new(),
        removed: Vec::new(),
        updated: Vec::new(),
    });
    let store = SqliteStore::open(workspace).expect("open store");
    let mut structural = StructuralIndexer::new(workspace.to_path_buf()).expect("structural");
    let synced = sync_snapshot(
        workspace,
        &store,
        &mut structural,
        &SnapshotScope::of(&observer),
        &events,
        &symbols_by_id,
    );
    assert!(synced.is_err(), "the structural pass must fail");
    assert!(leaf(&store, &removed.id).is_none(), "the prune committed");
    assert_eq!(rollup_status(&store, MODULE).as_deref(), Some("stale"));
    assert_eq!(rollup_status(&store, GONE), None);
    drop(store);

    // The next run finds nothing left to prune, and must not need to.
    index_once(workspace, false);
    let store = SqliteStore::open(workspace).expect("open store");
    assert_eq!(rollup_status(&store, MODULE).as_deref(), Some("stale"));
    assert_eq!(rollup_status(&store, GONE), None);
}

#[test]
fn full_index_once_leaves_a_leaf_that_failed_on_an_unchanged_body_alone() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let keep = named(&parse(workspace, MODULE), "keep");
    {
        // A deep SIR whose last regeneration failed: `stale`, the body unchanged.
        let store = SqliteStore::open(workspace).expect("open store");
        let mut meta = store
            .get_sir_meta(&keep.id)
            .expect("meta")
            .expect("keep SIR");
        meta.sir_status = "stale".to_owned();
        meta.generation_pass = "deep".to_owned();
        meta.last_error = Some("provider timeout".to_owned());
        store.upsert_sir_meta(meta).expect("record the failure");
    }

    index_once(workspace, true);

    let store = SqliteStore::open(workspace).expect("open store");
    let meta = store
        .get_sir_meta(&keep.id)
        .expect("meta")
        .expect("keep SIR");
    assert_eq!(
        meta.generation_pass, "deep",
        "a scan pass must not overwrite the deep SIR of an unchanged body"
    );
    assert_eq!(meta.sir_status, "stale");
}

#[test]
fn index_once_keeps_the_symbols_of_a_file_it_could_not_read() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let vanish = named(&parse(workspace, GONE), "vanish");

    // Still there, but no longer readable as text.
    let mut bytes = GONE_V1.as_bytes().to_vec();
    bytes.extend_from_slice(b"# \xff\n");
    fs::write(workspace.join(GONE), bytes).expect("corrupt second module");

    for full in [false, true] {
        index_once(workspace, full);
        let store = SqliteStore::open(workspace).expect("open store");
        assert!(
            stored_symbol_ids(&store).contains(&vanish.id),
            "a symbol of an unreadable file must not be pruned (full: {full})"
        );
        assert!(leaf(&store, &vanish.id).is_some());
        assert!(rollup(&store, GONE).is_some());
    }
}

#[test]
fn a_snapshot_never_undoes_what_was_indexed_after_it_began() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);

    // A run reads the workspace...
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let (events, symbols_by_id, _) = collect_initial_snapshot(&observer);
    // (the daemon's writes below land after the snapshot began, in a later second)
    let scope = SnapshotScope {
        taken_at: observer.seeded_at() - 1,
        ..SnapshotScope::of(&observer)
    };

    // ...while a daemon indexes edits made in the meantime.
    fs::write(workspace.join(MODULE), MODULE_EDITED_BY_DAEMON).expect("edit module");
    index_once(workspace, true);
    let daemon_keep = named(&parse(workspace, MODULE), "keep");
    let daemon_edited = named(&parse(workspace, MODULE), "edited");

    let store = SqliteStore::open(workspace).expect("open store");
    let mut structural = StructuralIndexer::new(workspace.to_path_buf()).expect("structural");
    sync_snapshot(
        workspace,
        &store,
        &mut structural,
        &scope,
        &events,
        &symbols_by_id,
    )
    .expect("sync the older snapshot");

    let edited = leaf(&store, &daemon_edited.id).expect("the daemon's SIR for edited");
    assert_eq!(
        edited.source_hash.as_deref(),
        Some(daemon_edited.content_hash.as_str()),
        "the newer symbol must not be migrated back onto its old id"
    );
    let keep = leaf(&store, &daemon_keep.id).expect("the daemon's SIR for keep");
    assert_eq!(
        keep.status, "fresh",
        "a SIR written for the newer body must not be marked stale by an older snapshot"
    );
}
