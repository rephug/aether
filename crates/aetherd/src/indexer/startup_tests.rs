//! The other snapshot syncs against a store indexed before the workspace changed: the
//! watching daemon's startup, and `run_structural_index_once` (batch extract and the
//! continuous monitor).

use std::fs;
use std::path::Path;

use aether_config::InferenceProviderKind;
use aether_infer::ProviderOverrides;
use aether_store::{SirStateStore, SqliteStore};
use tempfile::tempdir;

use super::index_once_tests::{
    GONE, MODULE, MODULE_BODY_EDIT, MODULE_WITHOUT_REMOVED, index_once, leaf, named, parse, rollup,
    rollup_id, stored_symbol_ids, write_workspace,
};
use super::structural::StructuralIndexer;
use super::worker::{SharedQueueState, refresh_pending_rollups};
use super::{
    ObserverState, SIR_GENERATION_PASS_SCAN, SirPipeline, run_structural_index_once,
    start_from_snapshot,
};

/// Run the daemon's startup against the workspace as it is on disk now.
fn start_daemon(workspace: &Path) -> (SqliteStore, SharedQueueState) {
    let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
    observer.seed_from_disk().expect("seed observer");
    let store = SqliteStore::open(workspace).expect("open store");
    let mut structural = StructuralIndexer::new(workspace.to_path_buf()).expect("structural");
    let queue_state = start_from_snapshot(workspace, &observer, &store, &mut structural)
        .expect("start from snapshot");
    (store, queue_state)
}

#[test]
fn daemon_startup_queues_symbols_edited_while_it_was_down() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let keep = named(&parse(workspace, MODULE), "keep");
    let vanished = named(&parse(workspace, GONE), "vanish");
    {
        // `keep` is unchanged but its last regeneration failed: it stays `stale` and
        // keeps its SIR rather than being queued for a scan-level replacement.
        let store = SqliteStore::open(workspace).expect("open store");
        let mut meta = store
            .get_sir_meta(&keep.id)
            .expect("meta")
            .expect("keep SIR");
        meta.sir_status = "stale".to_owned();
        meta.last_error = Some("provider timeout".to_owned());
        store.upsert_sir_meta(meta).expect("record the failure");
    }

    fs::write(workspace.join(MODULE), MODULE_BODY_EDIT).expect("edit body");
    fs::remove_file(workspace.join(GONE)).expect("delete second module");
    let edited = named(&parse(workspace, MODULE), "edited");

    let (store, queue_state) = start_daemon(workspace);

    let queue = queue_state.lock_queue();
    assert!(
        queue.contains(&edited.id),
        "a symbol edited while the daemon was down must be regenerated"
    );
    assert!(
        !queue.contains(&keep.id),
        "an unchanged symbol is not queued, even one a failed attempt left stale"
    );
    drop(queue);
    assert!(
        queue_state.take_pending_rollup_chunk(usize::MAX).is_none(),
        "the module's rollup is rebuilt with its queued leaf, not set aside"
    );
    assert_eq!(leaf(&store, &edited.id).expect("SIR").status, "stale");
    assert!(
        !stored_symbol_ids(&store).contains(&vanished.id),
        "a symbol deleted while the daemon was down must be pruned"
    );
    assert!(leaf(&store, &vanished.id).is_none());
}

#[test]
fn daemon_startup_rebuilds_outdated_rollups_once_the_queue_drains() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);

    fs::write(workspace.join(MODULE), MODULE_WITHOUT_REMOVED).expect("remove a function");
    let (store, queue_state) = start_daemon(workspace);
    assert_eq!(
        store
            .get_sir_meta(&rollup_id(MODULE))
            .expect("meta")
            .expect("rollup meta")
            .sir_status,
        "stale"
    );

    // No leaf of the file changed, so nothing rebuilds its rollup but the workers.
    let pending = queue_state
        .take_pending_rollup_chunk(usize::MAX)
        .expect("the rollup of the file that lost a symbol is set aside");
    assert!(pending.contains_key(MODULE));
    let pipeline = SirPipeline::new(
        workspace.to_path_buf(),
        1,
        ProviderOverrides {
            provider: Some(InferenceProviderKind::Mock),
            ..ProviderOverrides::default()
        },
    )
    .expect("pipeline");
    let rebuilt = refresh_pending_rollups(&pipeline, &store, pending, SIR_GENERATION_PASS_SCAN)
        .expect("rebuild rollups");
    assert_eq!(rebuilt, 1);
    assert_eq!(rollup(&store, MODULE).expect("rollup").symbol_count, 2);
    assert_eq!(
        store
            .get_sir_meta(&rollup_id(MODULE))
            .expect("meta")
            .expect("rollup meta")
            .sir_status,
        "fresh"
    );
    assert!(queue_state.take_pending_rollup_chunk(usize::MAX).is_none());
}

#[test]
fn structural_index_once_for_batch_and_monitor_syncs_the_snapshot() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_workspace(workspace);
    index_once(workspace, true);
    let vanished = named(&parse(workspace, GONE), "vanish");

    fs::write(workspace.join(MODULE), MODULE_BODY_EDIT).expect("edit body");
    fs::remove_file(workspace.join(GONE)).expect("delete second module");
    let edited = named(&parse(workspace, MODULE), "edited");

    let (store, symbols_by_id, _) = run_structural_index_once(workspace).expect("extract");

    assert!(symbols_by_id.contains_key(&edited.id));
    assert_eq!(leaf(&store, &edited.id).expect("SIR").status, "stale");
    assert!(!stored_symbol_ids(&store).contains(&vanished.id));
    assert!(
        store
            .get_sir_meta(&rollup_id(GONE))
            .expect("meta")
            .is_none(),
        "the rollup of a deleted file must be removed"
    );
}
