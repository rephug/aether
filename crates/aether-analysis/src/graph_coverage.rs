//! Resolution counts describe recorded call references, never whole-program recall.
use std::path::PathBuf;

use aether_store::{SqliteStore, StoreError};
use serde::{Deserialize, Serialize};

pub(crate) const OBSERVED_DISCONNECTION_NOTE: &str = "Disconnected from the largest observed dependency component; unresolved relationships may connect it. This does not establish unused or dead code";

const COVERAGE_NOTE: &str = "Observed graph only. Stored call counts are not recall; missing links cannot prove complete caller coverage or unused code.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedGraphStatus {
    Incomplete,
    #[default]
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservedGraphCoverage {
    pub status: ObservedGraphStatus,
    pub whole_program_completeness: String,
    pub call_edges: Option<u64>,
    pub resolved: Option<u64>,
    pub ambiguous: Option<u64>,
    pub unresolved: Option<u64>,
    pub file_level_calls: Option<u64>,
    pub note: String,
}

impl Default for ObservedGraphCoverage {
    fn default() -> Self {
        Self {
            status: ObservedGraphStatus::Unknown,
            whole_program_completeness: "unknown".to_owned(),
            call_edges: None,
            resolved: None,
            ambiguous: None,
            unresolved: None,
            file_level_calls: None,
            note: COVERAGE_NOTE.to_owned(),
        }
    }
}

impl ObservedGraphCoverage {
    /// Async callers must not lock or query SQLite on their executor thread.
    /// Reopen the same committed catalog read-only on a blocking worker: the
    /// borrowed handle cannot be moved into a static blocking task.
    pub async fn from_store_async(store: &SqliteStore) -> Self {
        let workspace = match graph_store_workspace(store) {
            Ok(workspace) => workspace,
            Err(err) => return Self::unavailable(err),
        };
        Self::from_workspace_async(workspace).await
    }

    async fn from_workspace_async(workspace: PathBuf) -> Self {
        match with_readonly_graph_store(workspace, |store| {
            Ok::<_, StoreError>(Self::from_store(store))
        })
        .await
        {
            Ok(coverage) => coverage,
            Err(err) => Self::unavailable(format!("call-reference coverage worker failed: {err}")),
        }
    }

    /// Graph data may remain usable when the separate count query is unavailable.
    pub fn from_store(store: &SqliteStore) -> Self {
        match store.call_edge_coverage_by_language() {
            Ok(rows) => {
                let (mut calls, mut resolved, mut ambiguous, mut unresolved, mut file_calls) =
                    (0, 0, 0, 0, 0);
                for row in rows {
                    calls += row.symbol_calls.call_edges;
                    resolved += row.symbol_calls.resolved;
                    ambiguous += row.symbol_calls.ambiguous;
                    unresolved += row.symbol_calls.unresolved;
                    file_calls += row.file_level_calls;
                }
                Self {
                    status: if ambiguous + unresolved + file_calls > 0 {
                        ObservedGraphStatus::Incomplete
                    } else {
                        ObservedGraphStatus::Unknown
                    },
                    call_edges: Some(calls),
                    resolved: Some(resolved),
                    ambiguous: Some(ambiguous),
                    unresolved: Some(unresolved),
                    file_level_calls: Some(file_calls),
                    ..Self::default()
                }
            }
            Err(err) => Self {
                status: ObservedGraphStatus::Unavailable,
                note: format!("Stored call-reference coverage unavailable: {err}. {COVERAGE_NOTE}"),
                ..Self::default()
            },
        }
    }

    pub fn unavailable(reason: impl std::fmt::Display) -> Self {
        Self {
            status: ObservedGraphStatus::Unavailable,
            note: format!("Observed dependency graph unavailable: {reason}. {COVERAGE_NOTE}"),
            ..Self::default()
        }
    }
}

/// Own the catalog path before dispatch so neither opening nor SQLite work can
/// block the async executor. Read-only opening never upgrades catalog data.
pub(crate) async fn with_readonly_graph_store<T, E>(
    workspace: PathBuf,
    operation: impl FnOnce(&SqliteStore) -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<StoreError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let store = SqliteStore::open_readonly(workspace).map_err(E::from)?;
        operation(&store)
    })
    .await
    .map_err(|err| {
        E::from(StoreError::Graph(format!(
            "read-only graph metadata worker failed: {err}"
        )))
    })?
}

pub(crate) fn graph_store_workspace(store: &SqliteStore) -> Result<PathBuf, StoreError> {
    store.workspace_root().ok_or_else(|| {
        StoreError::Compatibility("Graph metadata workspace path unavailable".to_owned())
    })
}

pub(crate) async fn require_native_graph_projection_async(
    store: &SqliteStore,
) -> Result<(), StoreError> {
    with_readonly_graph_store(
        graph_store_workspace(store)?,
        require_native_graph_projection,
    )
    .await
}

/// Entry-point check for native analyses handed an already-open graph.
pub(crate) fn require_native_graph_projection(
    store: &SqliteStore,
) -> Result<(), aether_store::StoreError> {
    if store.has_graph_projection_provenance()? {
        Ok(())
    } else {
        Err(aether_store::StoreError::Compatibility(
            "Persistent dependency graph proof is unavailable; run writable indexing/startup and structural reindex before reading graph relationships".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{SymbolCatalogStore, SymbolRecord, SymbolRelationStore};

    #[test]
    fn zero_and_fully_resolved_references_never_claim_completeness() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("store");
        let empty = ObservedGraphCoverage::from_store(&store);
        assert_eq!(empty.status, ObservedGraphStatus::Unknown);
        assert_eq!(empty.call_edges, Some(0));
        assert_eq!(empty.whole_program_completeness, "unknown");
        for id in ["caller", "helper"] {
            store
                .upsert_symbol(SymbolRecord {
                    id: id.to_owned(),
                    file_path: "src/lib.rs".to_owned(),
                    language: "rust".to_owned(),
                    kind: "function".to_owned(),
                    qualified_name: id.to_owned(),
                    signature_fingerprint: id.to_owned(),
                    last_seen_at: 1,
                })
                .expect("symbol");
        }
        let edge = |name: &str, target: Option<&str>| SymbolEdge {
            source_id: "caller".to_owned(),
            target_qualified_name: name.to_owned(),
            target_symbol_id: target.map(str::to_owned),
            target_file_path: None,
            edge_kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_owned(),
        };
        store
            .upsert_edges(&[edge("helper", Some("helper"))])
            .expect("proved call");
        let proved = ObservedGraphCoverage::from_store(&store);
        assert_eq!(proved.status, ObservedGraphStatus::Unknown);
        assert_eq!(proved.resolved, Some(1));
        assert_eq!(proved.unresolved, Some(0));
        assert_eq!(proved.whole_program_completeness, "unknown");
        let mut file_call = edge("helper", Some("helper"));
        file_call.source_id = aether_core::file_source_id("src/lib.rs");
        store.upsert_edges(&[file_call]).expect("file-level call");
        let file_level = ObservedGraphCoverage::from_store(&store);
        assert_eq!(file_level.status, ObservedGraphStatus::Incomplete);
        assert_eq!(file_level.call_edges, Some(1));
        assert_eq!(file_level.file_level_calls, Some(1));
        store
            .upsert_edges(&[edge("unknown", None)])
            .expect("unresolved call");
        let partial = ObservedGraphCoverage::from_store(&store);
        assert_eq!(partial.status, ObservedGraphStatus::Incomplete);
        assert_eq!(partial.resolved, Some(1));
        assert_eq!(partial.unresolved, Some(1));
    }

    #[test]
    fn sqlite_contention_does_not_block_the_current_thread_executor() {
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("store");
        store
            .upsert_symbol(SymbolRecord {
                id: "caller".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "caller".to_owned(),
                signature_fingerprint: "caller".to_owned(),
                last_seen_at: 1,
            })
            .expect("symbol");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: "caller".to_owned(),
                target_qualified_name: "callback".to_owned(),
                target_symbol_id: None,
                target_file_path: None,
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            }])
            .expect("raw call");
        let expected = ObservedGraphCoverage::from_store(&store);
        let workspace = store.workspace_root().expect("workspace");
        let catalog = store.aether_dir().join("meta.sqlite");
        drop(store);

        // DELETE mode makes an exclusive transaction block an actual read-only
        // catalog open/query. No connection remains open before changing mode.
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let locker = std::thread::spawn(move || {
            let connection = rusqlite::Connection::open(catalog).expect("catalog");
            connection
                .execute_batch("PRAGMA journal_mode = DELETE; BEGIN EXCLUSIVE;")
                .expect("exclusive catalog lock");
            locked_tx.send(()).expect("locked signal");
            // A broken inline implementation must fail, not deadlock the suite.
            let released = release_rx.recv_timeout(Duration::from_secs(10)).is_ok();
            connection.execute_batch("ROLLBACK").expect("release lock");
            released
        });
        locked_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("database locked");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let actual = runtime.block_on(async move {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let collection = tokio::spawn(async move {
                started_tx.send(()).expect("collection started");
                ObservedGraphCoverage::from_workspace_async(workspace).await
            });
            started_rx.await.expect("collection scheduled");
            // If opening/querying ran inline, this executor could not poll the
            // signal until the query timed out or the locker watchdog released.
            assert!(
                !collection.is_finished(),
                "SQLite contention must not occupy the async executor"
            );
            release_tx.send(()).expect("executor releases database");
            tokio::time::timeout(Duration::from_secs(10), collection)
                .await
                .expect("collection completes after release")
                .expect("collection task")
        });
        assert!(
            locker.join().expect("locker"),
            "watchdog must not release lock"
        );
        assert_eq!(actual, expected);
        assert_eq!(actual.unresolved, Some(1));
        assert_eq!(actual.whole_program_completeness, "unknown");
    }

    #[test]
    fn async_coverage_preserves_counts_and_metadata_failures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("store");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert_eq!(
            runtime.block_on(ObservedGraphCoverage::from_store_async(&store)),
            ObservedGraphCoverage::from_store(&store)
        );
        let connection =
            rusqlite::Connection::open(store.aether_dir().join("meta.sqlite")).expect("connection");
        connection
            .execute(
                "UPDATE schema_version SET version = 3 WHERE component = 'rust_target_provenance'",
                [],
            )
            .expect("future proof policy");
        let future = runtime.block_on(ObservedGraphCoverage::from_store_async(&store));
        assert_eq!(future.status, ObservedGraphStatus::Unavailable);
        assert_eq!(future.call_edges, None);
        assert!(
            future
                .note
                .contains("unsupported Rust target-proof version 3")
        );
        let guard_error = runtime
            .block_on(require_native_graph_projection_async(&store))
            .expect_err("future policy rejected");
        assert!(
            guard_error
                .to_string()
                .contains("unsupported Rust target-proof version 3")
        );
        let receipt: i64 = connection
            .query_row(
                "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
                [],
                |row| row.get(0),
            )
            .expect("receipt unchanged");
        assert_eq!(receipt, 3, "read-only helper must not migrate metadata");
        let absent = temp.path().join("absent");
        let missing = runtime.block_on(ObservedGraphCoverage::from_workspace_async(absent.clone()));
        assert_eq!(missing.status, ObservedGraphStatus::Unavailable);
        assert_eq!(missing.call_edges, None);
        assert!(!absent.join(".aether/meta.sqlite").exists());
    }

    #[test]
    fn unavailable_is_distinct_from_usable_unknown_graph() {
        let coverage = ObservedGraphCoverage::unavailable("reindex_required");
        assert_eq!(coverage.status, ObservedGraphStatus::Unavailable);
        assert_eq!(coverage.call_edges, None);
        assert!(coverage.note.contains("reindex_required"));
    }
}
