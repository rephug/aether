//! Retained canonical rows must survive projection rebuilds even when their source
//! cannot produce a current event. These tests use a fresh native graph, not the
//! SQLite graph backend whose nodes are already the canonical catalog.

use std::collections::BTreeSet;
use std::fs;

use aether_store::{SirHistoryStore, SirStateStore, SqliteStore, SymbolCatalogStore};

use super::index_once_tests::index_once;
use super::reconciliation::SnapshotScope;
use super::snapshot::sync_snapshot;
use super::structural::StructuralIndexer;
use super::{ObserverState, collect_initial_snapshot};

const RETAINED: &str = "retained.rs";
const CLEAN: &str = r#"
pub fn target() { kept(); }
pub fn kept() {}
pub struct Secret;
impl Secret {
    pub fn new() -> Self { Self }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("secret")
    }
}
"#;

fn rebuild_with_retained_source(quarantine: bool) {
    let temp = tempfile::tempdir().expect("workspace");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create store directory");
    let config = r#"[inference]
provider = "mock"
[storage]
mirror_sir_files = true
graph_backend = "sqlite"
[embeddings]
enabled = false
vector_backend = "sqlite"
"#;
    fs::write(workspace.join(".aether/config.toml"), config).expect("config");
    fs::write(workspace.join(RETAINED), CLEAN).expect("initial retained file");
    fs::write(
        workspace.join("caller.rs"),
        "pub fn caller() { target(); }\n",
    )
    .expect("caller");
    index_once(workspace, true);
    let store = SqliteStore::open(workspace).expect("store");
    let retained = store
        .list_symbols_for_file(RETAINED)
        .expect("retained rows");
    assert!(!retained.is_empty());
    let prior_read = store.file_read_at_ms(RETAINED).expect("prior read");
    let marker = crate::freshness::read_index_sync_marker(workspace);
    let evidence = retained
        .iter()
        .map(|row| {
            (
                row.id.clone(),
                store.read_sir_blob(&row.id).expect("prior payload"),
                store.list_sir_history(&row.id).expect("prior history"),
            )
        })
        .collect::<Vec<_>>();

    if quarantine {
        fs::write(
            workspace.join(RETAINED),
            format!(
                "{CLEAN}\nimpl std::fmt::Display for Secret {{ \
                 fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ \
                 f.write_str(\"secret\") }} }}"
            ),
        )
        .expect("legal Rust identity collision");
    } else {
        let mut bytes = CLEAN.as_bytes().to_vec();
        bytes.extend_from_slice(b"// \xff\n");
        fs::write(workspace.join(RETAINED), bytes).expect("unreadable UTF-8 source");
    }
    fs::write(
        workspace.join(".aether/config.toml"),
        config.replace("graph_backend = \"sqlite\"", "graph_backend = \"surreal\""),
    )
    .expect("switch only the synthetic fixture to a fresh native graph");

    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("partial snapshot");
    assert!(!SnapshotScope::of(&observer).read_file(RETAINED));
    assert_eq!(observer.blocked_sources().len(), usize::from(quarantine));
    let (events, symbols, _) = collect_initial_snapshot(&observer);
    assert!(events.iter().all(|event| event.file_path != RETAINED));
    let mut structural = StructuralIndexer::new(workspace.to_path_buf()).expect("fresh graph");
    // First native open now rebuilds directly from canonical proof before any
    // observer events are replayed. It must not claim to have read retained source.
    let bootstrap_ids = structural
        .graph_runtime
        .block_on(
            structural
                .surreal_graph_store
                .as_ref()
                .expect("native")
                .list_all_symbol_ids(),
        )
        .expect("bootstrapped graph")
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        bootstrap_ids,
        store
            .list_all_symbol_ids()
            .expect("canonical identities")
            .into_iter()
            .collect()
    );
    assert_eq!(
        store.file_read_at_ms(RETAINED).expect("retained read"),
        prior_read
    );
    assert_eq!(crate::freshness::read_index_sync_marker(workspace), marker);

    for _ in 0..2 {
        sync_snapshot(
            workspace,
            &store,
            &mut structural,
            &SnapshotScope::of(&observer),
            &events,
            &symbols,
        )
        .expect("rebuild/replay");
        let native = structural
            .surreal_graph_store
            .as_ref()
            .expect("native graph");
        let native_ids = structural
            .graph_runtime
            .block_on(native.list_all_symbol_ids())
            .expect("native identities")
            .into_iter()
            .collect::<BTreeSet<_>>();
        let canonical_ids = store
            .list_all_symbol_ids()
            .expect("canonical identities")
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(native_ids, canonical_ids, "retained rows need native nodes");
        let constructor = retained
            .iter()
            .find(|row| row.qualified_name == "target")
            .expect("retained target");
        let callers = store.list_symbols_for_file("caller.rs").expect("callers");
        let helper = retained
            .iter()
            .find(|row| row.qualified_name == "kept")
            .expect("retained dependency");
        let caller = callers
            .iter()
            .find(|row| row.qualified_name == "caller")
            .expect("caller");
        let canonical_incoming = store
            .list_symbol_edges_for_target_and_kinds("target", &[aether_core::EdgeKind::Calls])
            .expect("canonical caller edge");
        assert!(
            canonical_incoming.iter().any(|edge| {
                edge.source_id == caller.id
                    && edge.target_symbol_id.is_none()
                    && edge.target_file_path.is_none()
            }),
            "the caller's unimported spelling remains raw evidence"
        );
        let canonical_outgoing = store
            .list_symbol_edges_for_target_and_kinds("kept", &[aether_core::EdgeKind::Calls])
            .expect("canonical retained edge");
        assert!(
            canonical_outgoing
                .iter()
                .any(|edge| edge.source_id == constructor.id)
        );
        let outgoing = structural
            .graph_runtime
            .block_on(structural.graph_store.get_dependencies(&constructor.id))
            .expect("retained outgoing dependencies");
        assert!(outgoing.iter().any(|row| row.id == helper.id));
        let incoming = structural
            .graph_runtime
            .block_on(structural.graph_store.get_dependencies(&caller.id))
            .expect("caller dependencies");
        assert!(
            incoming.iter().all(|row| row.id != constructor.id),
            "an unimported same spelling in another file does not prove a call"
        );
        assert_eq!(
            store.list_symbols_for_file(RETAINED).expect("prior rows"),
            retained
        );
        assert_eq!(
            store.file_read_at_ms(RETAINED).expect("unchanged read"),
            prior_read
        );
        assert_eq!(crate::freshness::read_index_sync_marker(workspace), marker);
        for (id, payload, history) in &evidence {
            assert_eq!(&store.read_sir_blob(id).expect("payload"), payload);
            assert_eq!(&store.list_sir_history(id).expect("history"), history);
        }
    }
    drop(structural);
    let reopened = StructuralIndexer::new(workspace.to_path_buf()).expect("cold graph reopen");
    let native_ids = reopened
        .graph_runtime
        .block_on(
            reopened
                .surreal_graph_store
                .as_ref()
                .expect("native")
                .list_all_symbol_ids(),
        )
        .expect("reopened native identities")
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        native_ids,
        store
            .list_all_symbol_ids()
            .expect("canonical identities")
            .into_iter()
            .collect()
    );
    if quarantine {
        assert!(
            crate::indexing_gaps::read_summary(workspace)
                .expect("quarantine receipt")
                .unwrap_or_default()
                .contains("1 quarantined")
        );
    }
}

#[test]
fn fresh_graph_replays_quarantined_canonical_rows_without_claiming_freshness() {
    rebuild_with_retained_source(true);
}

#[test]
fn fresh_graph_replays_unreadable_canonical_rows_without_claiming_freshness() {
    rebuild_with_retained_source(false);
}
