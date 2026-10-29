//! Bringing the store in line with a structural snapshot taken without having watched
//! the workspace change: a one-shot index run, or the daemon's startup. The store may
//! have been indexed before the workspace changed, so symbols that no longer exist are
//! pruned (or their SIR migrated to the symbol's new id), every SIR that describes a
//! body its symbol no longer has is marked stale, and the rollups of the files that
//! changed are flagged or removed.

use aether_core::Language;

use super::reconciliation::{SnapshotScope, plan_symbol_reconciliation};
use super::structural::{StructuralIndexer, to_symbol_record};
use super::*;
use crate::sir_pipeline::{invalidate_file_rollups, invalidate_file_rollups_locked};

/// Sync the store to `events`, the workspace's structural snapshot (`symbols_by_id` is
/// the same snapshot keyed by id). `scope` says what the snapshot can speak for: stored
/// symbols of files it could not read, and rows and leaves written after it began (a
/// daemon indexing meanwhile), are left as they are. External projections of pruned
/// and migrated IDs are replayed from the durable retirement queue.
///
/// Catalog planning and publication share the inject lock. Replacement rows,
/// migrated annotations (marked stale), lineage, retirement evidence and cleanup
/// obligations commit together. Changed rollups are conservatively invalidated
/// before that commit, so an interrupted run can rebuild them. Mirrors, graph/edge
/// replacement, vector projection and freshness markers remain separate effects;
/// their global atomicity and unfenced older writers are not guaranteed here.
pub(super) fn sync_snapshot(
    workspace: &Path,
    store: &SqliteStore,
    structural: &mut StructuralIndexer,
    scope: &SnapshotScope<'_>,
    events: &[SymbolChangeEvent],
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<()> {
    crate::indexing_gaps::record(workspace, scope.taken_at, scope.blocked_sources)
        .context("record structural snapshot identity quarantine")?;
    // Fence injection and other structural writers before catalog reads/planning,
    // through atomic publication. External cleanup runs after releasing the lock.
    let inject_guard = acquire_inject_write_lock(workspace)?;
    // The ids the store held before this snapshot: reconciliation must not take an
    // unchanged symbol for the new identity of a stale one, and a current symbol not
    // among them is new.
    let pre_existing_ids = store
        .list_all_symbol_ids()
        .context("failed to list indexed symbols before the snapshot sync")?
        .into_iter()
        .collect::<HashSet<_>>();
    let current = current_sources(symbols_by_id);

    let plan = plan_symbol_reconciliation(store, symbols_by_id, &pre_existing_ids, scope)?;
    let changed_sources = store
        .list_sir_source_changes(&current, scope.taken_at)
        .context("failed to compare stored SIR sources with the snapshot")?
        .into_iter()
        .map(|(symbol_id, _)| symbol_id)
        .collect::<HashSet<_>>();
    // File path to language name, for the rollup ids of the files that changed.
    let mut changed_files = plan
        .removed_symbols()
        .map(|symbol| (symbol.file_path.clone(), symbol.language.clone()))
        .collect::<BTreeMap<_, _>>();
    for symbol in symbols_by_id.values() {
        if !pre_existing_ids.contains(symbol.id.as_str())
            || changed_sources.contains(symbol.id.as_str())
        {
            changed_files
                .entry(symbol.file_path.clone())
                .or_insert_with(|| symbol.language.as_str().to_owned());
        }
    }
    let pruned_ids = plan.pruned_ids();

    let now_ts = unix_timestamp_secs();
    let records = symbols_by_id
        .values()
        .map(|symbol| to_symbol_record(symbol, now_ts))
        .collect::<Vec<_>>();
    let (flagged, removed) = invalidate_file_rollups_locked(store, &changed_files, &pruned_ids)
        .context("failed to invalidate the rollups of changed files")?;
    if flagged + removed > 0 {
        tracing::info!(
            changed_files = changed_files.len(),
            flagged,
            removed,
            "Invalidated file rollups before canonical publication"
        );
    }
    plan.publish(store, &records)?;
    drop(inject_guard);

    // Replay even when this snapshot has no stale declarations: a previous
    // process can have committed retirement before its external deletion.
    crate::retirement_cleanup::replay_and_warn(workspace, store, &structural.graph_runtime)
        .context("replay persisted retirement work during snapshot startup")?;

    let stale_ids = store
        .mark_sir_stale_for_changed_sources(&current, scope.taken_at)
        .context("failed to mark the SIR of changed symbols stale")?;
    if !stale_ids.is_empty() {
        tracing::info!(
            stale = stale_ids.len(),
            "Marked SIR stale for {} symbols whose source changed since it was generated",
            stale_ids.len()
        );
    }
    // Only a leaf another writer replaced since step 1 can be marked without its file
    // being among the changed ones (a migrated leaf stays in its file); flag those too.
    let late_files = stale_ids
        .iter()
        .filter_map(|symbol_id| symbols_by_id.get(symbol_id))
        .filter(|symbol| !changed_files.contains_key(&symbol.file_path))
        .map(|symbol| {
            (
                symbol.file_path.clone(),
                symbol.language.as_str().to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    invalidate_file_rollups(workspace, store, &late_files)
        .context("failed to invalidate the rollups of changed files")?;

    for event in events {
        structural.process_event(store, event)?;
    }

    // The watcher diffs its next HEAD change against the HEAD from before the walk, not
    // HEAD now: a checkout during the walk happens before the watches exist, so only that
    // diff can still cover it.
    match scope.head {
        Some(head) => {
            let head = git_watch::HeadState {
                sha: head.commit.clone(),
                marker: head.git_ref.clone(),
            };
            if let Err(err) = git_watch::write_persisted_head_state(workspace, &head) {
                tracing::warn!(error = %err, "failed to record the watcher's HEAD");
            }
        }
        None => tracing::warn!(
            "HEAD could not be read before the walk; the watcher keeps its previous base"
        ),
    }

    // A file the index read before that is gone now has nothing left to describe. A
    // symbol-free one leaves edges and test intents that no symbol removal clears, so an
    // empty event clears them, and its read time is forgotten either way.
    match store.list_file_reads() {
        Ok(reads) => {
            let gone = reads
                .into_keys()
                .filter(|file_path| {
                    scope.read_file(file_path) && !workspace.join(file_path).exists()
                })
                .collect::<Vec<_>>();
            for file_path in &gone {
                if let Some(language) = language_for_path(Path::new(file_path)) {
                    structural.process_event(
                        store,
                        &SymbolChangeEvent {
                            file_path: file_path.clone(),
                            language,
                            added: Vec::new(),
                            removed: Vec::new(),
                            updated: Vec::new(),
                        },
                    )?;
                }
            }
            if let Err(err) = store.forget_file_reads(&gone) {
                tracing::warn!(error = %err, "failed to forget the reads of removed files");
            }
        }
        Err(err) => tracing::warn!(error = %err, "failed to list the files the index read"),
    }

    // Every file the snapshot read was read at some moment after its walk started; that
    // moment is the latest an edit can have reached it, so a later edit reads as a change.
    let read_files = symbols_by_id
        .values()
        .map(|symbol| symbol.file_path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if let Err(err) = store.record_file_reads(&read_files, scope.taken_at_millis) {
        tracing::warn!(error = %err, "failed to record when the snapshot read its files");
    }

    // Freshness may count this snapshot as a full sync only when it read every path: a
    // marker that claimed one it couldn't read would hide that path's changes. Without a
    // new marker, status keeps the previous commit and time, so it errs toward stale.
    if scope.unread_paths.is_empty() {
        if let Err(err) = crate::freshness::record_full_sync(
            workspace,
            scope.head_commit(),
            scope.taken_at,
            scope.taken_at_millis,
        ) {
            tracing::warn!(error = %err, "failed to record the index sync marker");
        }
    } else {
        tracing::warn!(
            unread_paths = scope.unread_paths.len(),
            "the snapshot could not read every path; the index sync marker keeps its previous \
             commit and time"
        );
    }

    Ok(())
}

/// Each symbol of the snapshot with the content hash of its body as parsed now.
fn current_sources(symbols_by_id: &HashMap<String, Symbol>) -> Vec<(String, String)> {
    symbols_by_id
        .values()
        .map(|symbol| (symbol.id.clone(), symbol.content_hash.clone()))
        .collect()
}

/// The symbols of `symbols_by_id` whose SIR must be regenerated although they have one:
/// those marked `stale` because their body changed since it was written. A leaf left
/// `stale` by a failed attempt on an unchanged body is not among them: regenerating it
/// at scan level would overwrite a better SIR (an injected or deep one) and spend a call
/// on every run while its own pass keeps failing.
pub(super) fn symbols_with_stale_source(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<Vec<String>> {
    Ok(store
        .list_sir_source_changes(&current_sources(symbols_by_id), i64::MAX)
        .context("failed to list leaves whose source changed")?
        .into_iter()
        .filter(|(_, status)| status == "stale")
        .map(|(symbol_id, _)| symbol_id)
        .collect())
}

/// The files of `symbols_by_id` whose rollup must be rebuilt (see `outdated_rollups`).
/// A full run rebuilds them after its scan; the files the scan regenerated a leaf of
/// had their rollup rebuilt by it and are `fresh` again.
pub(super) fn files_with_outdated_rollups(
    store: &SqliteStore,
    symbols_by_id: &HashMap<String, Symbol>,
) -> Result<BTreeMap<String, Language>> {
    let files = symbols_by_id
        .values()
        .map(|symbol| (symbol.file_path.clone(), symbol.language))
        .collect::<BTreeMap<_, _>>();
    outdated_rollups(store, files)
}

/// The files among `files` whose rollup must be rebuilt: those whose rollup is not
/// `fresh` (the snapshot sync flagged it, say), and those with leaf SIRs but no rollup
/// at all.
pub(super) fn outdated_rollups(
    store: &SqliteStore,
    files: BTreeMap<String, Language>,
) -> Result<BTreeMap<String, Language>> {
    let with_sir = store
        .list_symbol_ids_with_sir()
        .context("failed to list symbols with SIR")?
        .into_iter()
        .collect::<HashSet<_>>();

    let mut outdated = BTreeMap::new();
    for (file_path, language) in files {
        let rollup_id = synthetic_file_sir_id(language.as_str(), &file_path);
        let rollup = store
            .get_sir_meta(&rollup_id)
            .with_context(|| format!("failed to read the rollup of {file_path}"))?;
        let rebuild = match rollup {
            Some(meta) => !meta.sir_status.trim().eq_ignore_ascii_case("fresh"),
            None => store
                .list_symbols_for_file(&file_path)
                .with_context(|| format!("failed to list symbols for {file_path}"))?
                .iter()
                .any(|symbol| with_sir.contains(symbol.id.as_str())),
        };
        if rebuild {
            outdated.insert(file_path, language);
        }
    }
    Ok(outdated)
}
