//! Bringing the store in line with a structural snapshot taken without having watched
//! the workspace change: a one-shot index run, or the daemon's startup. The store may
//! have been indexed before the workspace changed, so symbols that no longer exist are pruned (or their SIR migrated to the
//! symbol's new id), every SIR that describes a body its symbol no longer has is marked
//! stale, and the rollups of the files that changed are flagged or removed.

use aether_core::Language;
use aether_store::open_vector_store;

use super::reconciliation::{execute_symbol_reconciliation, plan_symbol_reconciliation};
use super::structural::{StructuralIndexer, to_symbol_record};
use super::*;
use crate::sir_pipeline::{acquire_embed_write_locks, invalidate_file_rollups};

/// Sync the store to `events`, the workspace's structural snapshot (`symbols_by_id` is
/// the same snapshot keyed by id). `vector_cleanup` deletes the embeddings of pruned
/// and migrated ids.
///
/// The order matters. Every current symbol row is registered first, so reconciliation
/// can migrate a re-identified symbol's SIR onto the row of its new id. Reconciliation
/// then prunes the rows of symbols that are gone before the structural pass syncs each
/// file to the graph: that sync writes the file's symbol rows as graph nodes and
/// resolves edges against them by name, so a stale row still present would be written
/// and resolved to, only for reconciliation to delete it afterwards. Staleness is
/// decided last, against the rows as reconciliation left them, so a SIR migrated onto
/// a new id is compared with the new body too.
pub(super) fn sync_snapshot<VectorCleanup>(
    workspace: &Path,
    store: &SqliteStore,
    structural: &mut StructuralIndexer,
    events: &[SymbolChangeEvent],
    symbols_by_id: &HashMap<String, Symbol>,
    vector_cleanup: VectorCleanup,
) -> Result<()>
where
    VectorCleanup: FnOnce(&[String]) -> Result<()>,
{
    // The ids the store held before this snapshot: reconciliation must not take an
    // unchanged symbol for the new identity of a stale one, and a current symbol not
    // among them is new.
    let pre_existing_ids = store
        .list_all_symbol_ids()
        .context("failed to list indexed symbols before the snapshot sync")?
        .into_iter()
        .collect::<HashSet<_>>();

    let now_ts = unix_timestamp_secs();
    for symbol in symbols_by_id.values() {
        store
            .upsert_symbol(to_symbol_record(symbol, now_ts))
            .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
    }

    let plan = plan_symbol_reconciliation(store, symbols_by_id, &pre_existing_ids)?;
    // File path to language name, for the rollup ids of the files that changed.
    let mut changed_files = plan
        .removed_symbols()
        .map(|symbol| (symbol.file_path.clone(), symbol.language.clone()))
        .collect::<BTreeMap<_, _>>();
    execute_symbol_reconciliation(
        workspace,
        store,
        &plan,
        |symbol_ids| structural.delete_symbols_batch(symbol_ids),
        vector_cleanup,
    )?;

    for event in events {
        structural.process_event(store, event)?;
    }

    let current_sources = symbols_by_id
        .values()
        .map(|symbol| (symbol.id.clone(), symbol.content_hash.clone()))
        .collect::<Vec<_>>();
    let stale_ids = store
        .mark_sir_stale_for_changed_sources(&current_sources)
        .context("failed to mark the SIR of changed symbols stale")?
        .into_iter()
        .collect::<HashSet<_>>();
    if !stale_ids.is_empty() {
        tracing::info!(
            stale = stale_ids.len(),
            "Marked SIR stale for {} symbols whose source changed since it was generated",
            stale_ids.len()
        );
    }

    for symbol in symbols_by_id.values() {
        if !pre_existing_ids.contains(symbol.id.as_str()) || stale_ids.contains(&symbol.id) {
            changed_files
                .entry(symbol.file_path.clone())
                .or_insert_with(|| symbol.language.as_str().to_owned());
        }
    }
    let (flagged, removed) = invalidate_file_rollups(workspace, store, &changed_files)
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
