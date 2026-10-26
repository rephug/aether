//! Bounded daemon-side replay of external retirement projections.
//!
//! SQLite retirement and its SQLite projections are already atomic. These jobs
//! repair external Surreal/Lance deletions; they do not claim global generations
//! or transactional cross-store publication. No provider is initialized here.
use std::path::Path;

use aether_config::{GraphBackend, load_workspace_config};
use aether_store::{
    LanceRetirementCleanup, RetirementCleanupOutcome, RetirementCleanupStore,
    RetirementCleanupSummary, RetirementProjection, SqliteStore, StoreError, SurrealGraphStore,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::sir_pipeline::acquire_embed_write_lock;

const MAX_REPLAY_BATCH: u32 = 128;
pub const DEFAULT_REPLAY_BATCH: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementReplayPolicy {
    All,
    /// Inference-only regeneration/batch work must never open or drain graph jobs.
    VectorOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetirementReplayFailure {
    pub retirement_id: i64,
    pub projection: RetirementProjection,
    pub backend: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetirementReplayReport {
    pub attempted: u32,
    pub completed: u32,
    pub already_completed: u32,
    pub superseded_by_reintroduction: u32,
    pub failures: Vec<RetirementReplayFailure>,
    pub warnings: Vec<String>,
    pub remaining: RetirementCleanupSummary,
}

/// Replay at most 128 components. Pending selection orders by attempts/update time
/// so persistent failures do not monopolize later bounded batches. Each callback
/// deletes one ID; Lance additionally caps the prepared table set at 128.
///
/// Backend handles are initialized *before* the SQLite-fenced callback. Vector
/// cleanup acquires the embedding lock before preparing existing tables or entering
/// that fence. Preparation never imports SQLite data or initializes a missing store. No callback acquires another lock, reenters SQLite, initializes a
/// provider, runs a migration or opens a backend.
///
/// We deliberately do not cancel an in-flight delete on an arbitrary timer: a
/// backend that continues after cancellation could delete a reintroduced ID after
/// the catalog fence is released. A slow external operation can hold the writer
/// fence; wall-time deadlines need acknowledged backend cancellation/versioned
/// deletion, a separate follow-up beyond bounded component/table counts.
pub fn replay_retirement_cleanup(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &Runtime,
    limit: u32,
) -> Result<RetirementReplayReport> {
    replay_retirement_cleanup_with_policy(
        workspace,
        store,
        runtime,
        limit,
        RetirementReplayPolicy::All,
    )
}

pub fn replay_retirement_cleanup_with_policy(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &Runtime,
    limit: u32,
    policy: RetirementReplayPolicy,
) -> Result<RetirementReplayReport> {
    let pending = if limit == 0 {
        Vec::new()
    } else {
        match policy {
            RetirementReplayPolicy::All => {
                store.list_pending_retirement_cleanup(limit.min(MAX_REPLAY_BATCH))
            }
            RetirementReplayPolicy::VectorOnly => store
                .list_pending_retirement_cleanup_for_projection(
                    RetirementProjection::Vector,
                    limit.min(MAX_REPLAY_BATCH),
                ),
        }
        .context("read eligible pending retirement cleanup")?
    };
    let mut report = RetirementReplayReport {
        attempted: 0,
        completed: 0,
        already_completed: 0,
        superseded_by_reintroduction: 0,
        failures: Vec::new(),
        warnings: Vec::new(),
        remaining: store
            .retirement_cleanup_summary(10)
            .context("read retirement cleanup summary")?,
    };
    report.warnings = replay_warnings(&report.remaining, policy);
    if pending.is_empty() {
        return Ok(report);
    }
    let config = load_workspace_config(workspace).context("load retirement backend identities")?;
    let graph_backend = match config.storage.graph_backend {
        GraphBackend::Surreal | GraphBackend::Cozo => "surreal",
        GraphBackend::Sqlite => "sqlite",
    };
    let vector_backend = config.embeddings.vector_backend.as_str();
    // Initialize the graph once and cache Lance preparation failures per replay.
    // Successful Lance preparation snapshots existing tables per locked symbol;
    // failures still pass through the fenced API and are not silently skipped.
    let mut graph: Option<std::result::Result<SurrealGraphStore, String>> = None;
    let mut lance_preparation_error: Option<String> = None;
    for record in pending {
        report.attempted += 1;
        let result = match record.projection {
            RetirementProjection::Graph => {
                if record.backend == graph_backend && graph_backend == "surreal" && graph.is_none()
                {
                    graph = Some(
                        runtime
                            .block_on(SurrealGraphStore::open(workspace))
                            .map_err(|error| format!("prepare Surreal retirement handle: {error}")),
                    );
                }
                let mut cleanup = |symbol_id: &str| match graph.as_ref() {
                    Some(Ok(graph)) => {
                        runtime.block_on(graph.delete_symbols_batch(&[symbol_id.to_owned()]))
                    }
                    Some(Err(error)) => Err(StoreError::Graph(error.clone())),
                    None => Err(StoreError::Compatibility(
                        "no matching external graph cleanup handle".to_owned(),
                    )),
                };
                store.attempt_retirement_cleanup(
                    record.retirement_id,
                    record.projection,
                    graph_backend,
                    &mut cleanup,
                )
            }
            RetirementProjection::Vector => {
                // Failure to acquire the lock is itself recorded durably; that
                // callback performs no external operation, so no lock is needed.
                let guard = acquire_embed_write_lock(workspace, &record.symbol_id);
                let prepared = match &guard {
                    Err(error) => Err(format!("acquire retirement embedding lock: {error:#}")),
                    Ok(_) if record.backend != vector_backend || vector_backend != "lancedb" => {
                        Err("no matching external vector cleanup backend".to_owned())
                    }
                    Ok(_) => {
                        if let Some(error) = &lance_preparation_error {
                            Err(error.clone())
                        } else {
                            let prepared = runtime
                                .block_on(LanceRetirementCleanup::open_existing(workspace))
                                .map_err(|error| {
                                    format!("prepare existing Lance retirement tables: {error}")
                                });
                            if let Err(error) = &prepared {
                                lance_preparation_error = Some(error.clone());
                            }
                            prepared
                        }
                    }
                };
                let mut cleanup = |symbol_id: &str| match &prepared {
                    Ok(prepared) => runtime.block_on(prepared.delete_symbol(symbol_id)),
                    Err(error) => Err(StoreError::LanceDb(error.clone())),
                };
                let result = store.attempt_retirement_cleanup(
                    record.retirement_id,
                    record.projection,
                    vector_backend,
                    &mut cleanup,
                );
                drop(guard); // kept through both external deletion and acknowledgement
                result
            }
        };
        match result {
            Ok(RetirementCleanupOutcome::Completed) => report.completed += 1,
            Ok(RetirementCleanupOutcome::AlreadyCompleted) => report.already_completed += 1,
            Ok(RetirementCleanupOutcome::SupersededByReintroduction) => {
                report.superseded_by_reintroduction += 1
            }
            Ok(RetirementCleanupOutcome::LegacyUnknown) => {}
            Err(error) => report.failures.push(RetirementReplayFailure {
                retirement_id: record.retirement_id,
                projection: record.projection,
                backend: record.backend,
                error: error.to_string(),
            }),
        }
    }
    report.remaining = store
        .retirement_cleanup_summary(10)
        .context("read retirement cleanup summary after replay")?;
    report.warnings = replay_warnings(&report.remaining, policy);
    Ok(report)
}

fn replay_warnings(
    summary: &RetirementCleanupSummary,
    policy: RetirementReplayPolicy,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if !summary.available {
        warnings.push(
            "Retirement cleanup status is unknown: this store has no durable queue.".to_owned(),
        );
    }
    if summary.pending_graph > 0 || summary.pending_vector > 0 {
        warnings.push(format!("Retirement cleanup remains incomplete: {} graph and {} vector components pending after this bounded batch ({policy:?}). Indexing completion does not acknowledge this backlog.", summary.pending_graph, summary.pending_vector));
    }
    if summary.legacy_unknown_components > 0 {
        warnings.push(format!("{} retirement components have unknown historical backend identity and were not repaired.", summary.legacy_unknown_components));
    }
    if summary.superseded_components > 0 {
        warnings.push(format!("{} retirement components were superseded by same-ID reintroduction; ordinary projection reconstruction remains required before freshness is established.", summary.superseded_components));
    }
    warnings
}

/// Source updates continue with a visible backlog; a failed projection is never
/// acknowledged as done. Startup and periodic calls retry it without generation.
pub(crate) fn replay_and_warn(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &Runtime,
) -> Result<RetirementReplayReport> {
    replay_and_warn_with_policy(workspace, store, runtime, RetirementReplayPolicy::All)
}

pub(crate) fn replay_and_warn_with_policy(
    workspace: &Path,
    store: &SqliteStore,
    runtime: &Runtime,
    policy: RetirementReplayPolicy,
) -> Result<RetirementReplayReport> {
    let report = replay_retirement_cleanup_with_policy(
        workspace,
        store,
        runtime,
        DEFAULT_REPLAY_BATCH,
        policy,
    )?;
    for warning in &report.warnings {
        tracing::warn!(message = %warning, "Retirement projection limitation");
    }
    if !report.failures.is_empty() {
        tracing::warn!(failures = ?report.failures, "Retirement cleanup attempts failed");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_config::{AetherConfig, EmbeddingVectorBackend, save_workspace_config};
    use aether_store::{
        GraphStore, LanceVectorStore, SemanticIndexStore, SymbolCatalogStore,
        SymbolEmbeddingRecord, SymbolRecord, VectorStore,
    };

    fn runtime() -> Result<Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("build isolated cleanup runtime")
    }

    fn configure(
        workspace: &Path,
        graph: GraphBackend,
        vector: EmbeddingVectorBackend,
    ) -> Result<()> {
        let mut config = AetherConfig::default();
        config.storage.graph_backend = graph;
        config.embeddings.vector_backend = vector;
        config.embeddings.enabled = false;
        save_workspace_config(workspace, &config).context("write disposable cleanup configuration")
    }

    fn symbol() -> SymbolRecord {
        SymbolRecord {
            id: "retired".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "retired".to_owned(),
            signature_fingerprint: "old".to_owned(),
            last_seen_at: 1,
        }
    }

    fn vector(hash: &str) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: "retired".to_owned(),
            sir_hash: hash.to_owned(),
            provider: "fixture".to_owned(),
            model: "2d".to_owned(),
            embedding: vec![1.0, 0.0],
            updated_at: 1,
        }
    }

    #[test]
    fn reopening_store_replays_saved_lance_cleanup_without_generation() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Sqlite,
            EmbeddingVectorBackend::Lancedb,
        )?;
        {
            let runtime = runtime()?;
            let store = SqliteStore::open(workspace.path())?;
            let lance = runtime.block_on(LanceVectorStore::open(workspace.path()))?;
            store.upsert_symbol(symbol())?;
            runtime.block_on(lance.upsert_embedding(vector("old")))?;
            store.mark_removed("retired")?;
            assert_eq!(store.retirement_cleanup_summary(0)?.pending_vector, 1);
            assert!(
                runtime
                    .block_on(lance.get_embedding_meta("retired"))?
                    .is_some()
            );
            // Simulate daemon shutdown between SQLite retirement and external deletion.
        }
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        let report = replay_retirement_cleanup(workspace.path(), &store, &runtime, 1)?;
        assert_eq!(report.completed, 1);
        assert!(report.failures.is_empty());
        assert_eq!(report.remaining.pending_vector, 0);
        let lance = runtime.block_on(LanceVectorStore::open(workspace.path()))?;
        assert!(
            runtime
                .block_on(lance.get_embedding_meta("retired"))?
                .is_none()
        );
        assert_eq!(
            replay_retirement_cleanup(workspace.path(), &store, &runtime, 32)?.attempted,
            0
        );
        Ok(())
    }

    #[test]
    fn reintroduced_same_id_keeps_its_new_lance_vector() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Sqlite,
            EmbeddingVectorBackend::Lancedb,
        )?;
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        let lance = runtime.block_on(LanceVectorStore::open(workspace.path()))?;
        store.upsert_symbol(symbol())?;
        runtime.block_on(lance.upsert_embedding(vector("old")))?;
        store.mark_removed("retired")?;
        store.upsert_symbol(symbol())?;
        runtime.block_on(lance.upsert_embedding(vector("new-incarnation")))?;
        let report = replay_retirement_cleanup(workspace.path(), &store, &runtime, 32)?;
        assert_eq!(report.superseded_by_reintroduction, 1);
        assert!(report.failures.is_empty());
        assert_eq!(
            runtime
                .block_on(lance.get_embedding_meta("retired"))?
                .map(|meta| meta.sir_hash),
            Some("new-incarnation".to_owned())
        );
        Ok(())
    }

    #[test]
    fn backend_initialization_failure_is_saved_and_repaired_on_reopen() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Sqlite,
            EmbeddingVectorBackend::Lancedb,
        )?;
        {
            let runtime = runtime()?;
            let store = SqliteStore::open(workspace.path())?;
            store.upsert_symbol(symbol())?;
            store.mark_removed("retired")?;
            std::fs::write(
                workspace.path().join(".aether/vectors"),
                "fault: directory is a file",
            )?;
            let report = replay_retirement_cleanup(workspace.path(), &store, &runtime, 32)?;
            assert_eq!(report.failures.len(), 1);
            let pending = store.list_pending_retirement_cleanup(1)?;
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].attempts, 1);
            assert!(
                pending[0]
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("prepare existing Lance"))
            );
        }
        std::fs::remove_file(workspace.path().join(".aether/vectors"))?;
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        let report = replay_retirement_cleanup(workspace.path(), &store, &runtime, 32)?;
        assert_eq!(report.completed, 1);
        assert!(report.failures.is_empty());
        assert_eq!(report.remaining.pending_vector, 0);
        Ok(())
    }

    #[test]
    fn vector_only_replay_never_opens_or_attempts_the_pending_graph() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Surreal,
            EmbeddingVectorBackend::Lancedb,
        )?;
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        let lance = runtime.block_on(LanceVectorStore::open(workspace.path()))?;
        store.upsert_symbol(symbol())?;
        runtime.block_on(lance.upsert_embedding(vector("old")))?;
        store.mark_removed("retired")?;
        let graph_before = store
            .list_pending_retirement_cleanup(10)?
            .into_iter()
            .find(|row| row.projection == RetirementProjection::Graph)
            .context("pending graph fixture")?;
        assert!(!workspace.path().join(".aether/graph").exists());
        // Limit one would select the graph first without SQL-level eligibility filtering.
        let report = replay_retirement_cleanup_with_policy(
            workspace.path(),
            &store,
            &runtime,
            1,
            RetirementReplayPolicy::VectorOnly,
        )?;
        assert_eq!(report.completed, 1);
        assert_eq!(report.remaining.pending_graph, 1);
        assert_eq!(report.remaining.pending_vector, 0);
        let graph_after = store
            .list_pending_retirement_cleanup(10)?
            .into_iter()
            .find(|row| row.projection == RetirementProjection::Graph)
            .context("graph must remain pending")?;
        assert_eq!(graph_after, graph_before);
        assert!(!workspace.path().join(".aether/graph").exists());
        assert!(
            runtime
                .block_on(lance.get_embedding_meta("retired"))?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn successful_bounded_batch_reports_its_unfinished_backlog() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Sqlite,
            EmbeddingVectorBackend::Lancedb,
        )?;
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        for index in 0..DEFAULT_REPLAY_BATCH + 2 {
            let mut record = symbol();
            record.id = format!("retired-{index}");
            store.upsert_symbol(record.clone())?;
            store.mark_removed(&record.id)?;
        }
        let report = replay_and_warn(workspace.path(), &store, &runtime)?;
        assert_eq!(report.completed, DEFAULT_REPLAY_BATCH);
        assert!(report.failures.is_empty());
        assert_eq!(report.remaining.pending_vector, 2);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("0 graph and 2 vector components pending"))
        );
        assert!(
            !workspace.path().join(".aether/vectors").exists(),
            "cleanup must not initialize absent Lance stores"
        );
        Ok(())
    }

    #[test]
    fn cleanup_never_migrates_legacy_sqlite_rows_or_writes_markers() -> Result<()> {
        for reintroduce in [false, true] {
            let workspace = tempfile::tempdir()?;
            configure(
                workspace.path(),
                GraphBackend::Sqlite,
                EmbeddingVectorBackend::Lancedb,
            )?;
            let runtime = runtime()?;
            let store = SqliteStore::open(workspace.path())?;
            let lance = runtime.block_on(LanceVectorStore::open(workspace.path()))?;
            store.upsert_symbol(symbol())?;
            runtime.block_on(lance.upsert_embedding(vector("old-A")))?;
            let mut other = symbol();
            other.id = "unrelated-B".to_owned();
            other.qualified_name = "unrelated_B".to_owned();
            store.upsert_symbol(other)?;
            let mut current_b = vector("new-B");
            current_b.symbol_id = "unrelated-B".to_owned();
            runtime.block_on(lance.upsert_embedding(current_b.clone()))?;
            let mut legacy_b = current_b;
            legacy_b.sir_hash = "stale-B".to_owned();
            store.upsert_symbol_embedding(legacy_b)?;
            store.mark_removed("retired")?;
            if reintroduce {
                store.upsert_symbol(symbol())?;
            }
            let symbols_marker = workspace.path().join(".aether/vectors/.sqlite_migrated_v1");
            let notes_marker = workspace
                .path()
                .join(".aether/vectors/.sqlite_project_notes_migrated_v1");
            std::fs::remove_file(&symbols_marker)?;
            std::fs::remove_file(&notes_marker)?;
            let report = replay_retirement_cleanup_with_policy(
                workspace.path(),
                &store,
                &runtime,
                32,
                RetirementReplayPolicy::VectorOnly,
            )?;
            assert!(report.failures.is_empty());
            assert_eq!(report.superseded_by_reintroduction, u32::from(reintroduce));
            assert_eq!(report.completed, u32::from(!reintroduce));
            assert!(
                !symbols_marker.exists(),
                "cleanup created a symbol migration marker"
            );
            assert!(
                !notes_marker.exists(),
                "cleanup created a note migration marker"
            );
            // Only the fixture restores a marker after asserting cleanup's no-import
            // contract, so the ordinary inspection API does not itself migrate B.
            std::fs::write(&symbols_marker, "fixture inspection only")?;
            let b = runtime
                .block_on(lance.get_embedding_meta("unrelated-B"))?
                .context("B vector")?;
            assert_eq!(
                b.sir_hash, "new-B",
                "cleanup imported stale unrelated SQLite data"
            );
        }
        Ok(())
    }

    #[test]
    fn external_surreal_cleanup_uses_fenced_replay() -> Result<()> {
        let workspace = tempfile::tempdir()?;
        configure(
            workspace.path(),
            GraphBackend::Surreal,
            EmbeddingVectorBackend::Sqlite,
        )?;
        let runtime = runtime()?;
        let store = SqliteStore::open(workspace.path())?;
        let graph = runtime.block_on(SurrealGraphStore::open(workspace.path()))?;
        store.upsert_symbol(symbol())?;
        runtime.block_on(graph.upsert_symbol_node(&symbol()))?;
        store.mark_removed("retired")?;
        assert!(
            runtime
                .block_on(graph.list_all_symbol_ids())?
                .contains(&"retired".to_owned())
        );
        let report = replay_retirement_cleanup(workspace.path(), &store, &runtime, 32)?;
        assert_eq!(report.completed, 1);
        assert!(report.failures.is_empty());
        assert!(
            !runtime
                .block_on(graph.list_all_symbol_ids())?
                .contains(&"retired".to_owned())
        );
        assert_eq!(report.remaining.pending_graph, 0);
        Ok(())
    }
}
