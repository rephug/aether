//! Bringing the store in line with a structural snapshot taken without having watched
//! the workspace change: a one-shot index run, or the daemon's startup. The store may
//! have been indexed before the workspace changed, so symbols that no longer exist are
//! pruned (or their SIR migrated to the symbol's new id), every SIR that describes a
//! body its symbol no longer has is marked stale, and the rollups of the files that
//! changed are flagged or removed.

use aether_core::Language;
use aether_store::open_vector_store;

use super::reconciliation::{
    SnapshotScope, execute_symbol_reconciliation, plan_symbol_reconciliation,
};
use super::structural::{StructuralIndexer, to_symbol_record};
use super::*;
use crate::sir_pipeline::{
    acquire_embed_write_locks, invalidate_file_rollups, invalidate_file_rollups_locked,
};

/// Sync the store to `events`, the workspace's structural snapshot (`symbols_by_id` is
/// the same snapshot keyed by id). `scope` says what the snapshot can speak for: stored
/// symbols of files it could not read, and rows and leaves written after it began (a
/// daemon indexing meanwhile), are left as they are. `vector_cleanup` deletes the
/// embeddings of pruned and migrated ids.
///
/// The order matters, and every step is recorded before the next one can lose track of
/// it, so a run interrupted anywhere (a cancelled index, an error in the structural
/// pass) leaves nothing the next run cannot find again:
///
/// 1. Everything that changed is found while the store still shows it: the symbols to
///    prune or migrate, the symbols that are new, and the leaves whose recorded source
///    differs from the current body.
/// 2. Every current symbol row is registered, so reconciliation can migrate a
///    re-identified symbol's SIR onto the row of its new id.
/// 3. The rollups of the changed files are flagged (or removed, for a file about to lose
///    its last leaf) in the same inject-lock hold as the prune and before it commits: a
///    run that stopped after the prune would find nothing left to prune next time, and
///    would never revisit those files.
/// 4. Leaves whose source changed are marked stale, migrated ones included (their
///    recorded hash is the old body's). Stopped before this, they are still `fresh` with
///    an outdated source hash, which step 1 finds again.
/// 5. The structural pass syncs each file's edges and graph nodes last, after the prune,
///    so ids about to be pruned are never written to the graph or resolved to.
pub(super) fn sync_snapshot<VectorCleanup>(
    workspace: &Path,
    store: &SqliteStore,
    structural: &mut StructuralIndexer,
    scope: &SnapshotScope<'_>,
    events: &[SymbolChangeEvent],
    symbols_by_id: &HashMap<String, Symbol>,
    vector_cleanup: VectorCleanup,
) -> Result<()>
where
    VectorCleanup: FnOnce(&[String]) -> Result<()>,
{
    // The HEAD (and ref) the watcher diffs its next HEAD change against once the store
    // holds this snapshot.
    let head_at_start =
        git_watch::read_current_head_state(workspace, resolve_git_watch_dir(workspace).as_deref());

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
    store
        .upsert_symbols(&records)
        .context("failed to register the snapshot's symbols")?;

    execute_symbol_reconciliation(
        workspace,
        store,
        &plan,
        || {
            let (flagged, removed) =
                invalidate_file_rollups_locked(store, &changed_files, &pruned_ids)
                    .context("failed to invalidate the rollups of changed files")?;
            if flagged + removed > 0 {
                tracing::info!(
                    changed_files = changed_files.len(),
                    flagged,
                    removed,
                    "Invalidated file rollups of changed files: {flagged} marked stale, {removed} removed"
                );
            }
            Ok(())
        },
        |symbol_ids| structural.delete_symbols_batch(symbol_ids),
        vector_cleanup,
    )?;

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

    match head_at_start {
        Ok(head) => {
            if let Err(err) = git_watch::write_persisted_head_state(workspace, &head) {
                tracing::warn!(error = %err, "failed to record the watcher's HEAD");
            }
        }
        Err(err) => tracing::warn!(error = %err, "failed to read HEAD for the watcher"),
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
            scope.head,
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

/// Vector cleanup for a sync without a SIR pipeline (the structural `--index-once`, the
/// daemon's startup): the configured vector store is opened only when there is
/// something to delete, and the deletion holds each symbol's embedding lock as the
/// pipeline's does.
pub(super) fn delete_vector_embeddings(workspace: &Path, symbol_ids: &[String]) -> Result<()> {
    if symbol_ids.is_empty() {
        return Ok(());
    }
    let _embed_guards =
        acquire_embed_write_locks(workspace, symbol_ids.iter().map(String::as_str))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build vector cleanup runtime")?;
    runtime.block_on(async {
        let vector_store = open_vector_store(workspace)
            .await
            .context("failed to open the vector store")?;
        vector_store
            .delete_embeddings(symbol_ids)
            .await
            .context("failed to delete symbol embeddings")
    })
}
