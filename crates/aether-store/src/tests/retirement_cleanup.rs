use std::cell::Cell;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use aether_config::{AetherConfig, EmbeddingVectorBackend, GraphBackend, save_workspace_config};

use super::*;

fn cleanup_fixture(
    graph: GraphBackend,
    vector: EmbeddingVectorBackend,
) -> (SqliteStore, tempfile::TempDir) {
    let temp = tempdir().expect("tempdir");
    let mut config = AetherConfig::default();
    config.storage.graph_backend = graph;
    config.storage.mirror_sir_files = false;
    config.embeddings.vector_backend = vector;
    config.embeddings.enabled = false;
    save_workspace_config(temp.path(), &config).expect("fixture config");
    (SqliteStore::open(temp.path()).expect("store"), temp)
}

fn seed_retirement(store: &SqliteStore, id: &str) -> i64 {
    let mut record = symbol_record();
    record.id = id.to_owned();
    store.upsert_symbol(record).expect("symbol");
    upsert_sir_state(
        store,
        id,
        "saved-hash",
        r#"{"intent":"saved evidence"}"#,
        10,
    );
    store.mark_removed(id).expect("retire");
    store.list_symbol_retirements(id).expect("archive")[0].retirement_id
}

#[test]
fn cleanup_is_durable_per_backend_and_retries_without_mutating_archive() {
    let (store, temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "pending");
    let original = store
        .list_symbol_retirements("pending")
        .expect("original archive");
    let pending = store.list_pending_retirement_cleanup(10).expect("pending");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].backend, "surreal");
    assert_eq!(pending[1].backend, "lancedb");
    let mut failure = |_: &str| Err(StoreError::Graph("fixture backend unavailable".to_owned()));
    assert!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut failure
            )
            .is_err()
    );
    drop(store);
    let store = SqliteStore::open(temp.path()).expect("restart");
    let pending = store
        .list_pending_retirement_cleanup(10)
        .expect("restart pending");
    assert_eq!(pending[0].attempts, 1);
    assert!(
        pending[0]
            .last_error
            .as_ref()
            .expect("error")
            .contains("fixture backend unavailable")
    );
    let calls = Cell::new(0);
    let mut success = |id: &str| {
        assert_eq!(id, "pending");
        calls.set(calls.get() + 1);
        Ok(())
    };
    assert_eq!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut success
            )
            .expect("retry"),
        RetirementCleanupOutcome::Completed
    );
    assert_eq!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut success
            )
            .expect("repeat"),
        RetirementCleanupOutcome::AlreadyCompleted
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(
        store
            .list_symbol_retirements("pending")
            .expect("immutable archive"),
        original
    );
    let summary = store.retirement_cleanup_summary(1).expect("summary");
    assert_eq!(
        (
            summary.pending_graph,
            summary.pending_vector,
            summary.failed_components
        ),
        (0, 1, 0)
    );
    assert_eq!(summary.samples.len(), 1);
}

#[test]
fn cleanup_queue_and_archive_roll_back_when_retirement_fails() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    store.upsert_symbol(symbol_record()).expect("symbol");
    upsert_sir_state(
        &store,
        "sym-1",
        "saved-hash",
        r#"{"intent":"saved evidence"}"#,
        10,
    );
    store.conn.lock().expect("lock").execute_batch(
        "CREATE TRIGGER reject_cleanup BEFORE INSERT ON retirement_cleanup BEGIN SELECT RAISE(ABORT, 'injected queue failure'); END;",
    ).expect("trigger");
    assert!(store.mark_removed("sym-1").is_err());
    assert!(store.get_symbol_record("sym-1").expect("symbol").is_some());
    assert_eq!(store.list_sir_history("sym-1").expect("history").len(), 1);
    assert!(
        store
            .list_symbol_retirements("sym-1")
            .expect("archive")
            .is_empty()
    );
    assert!(
        store
            .list_pending_retirement_cleanup(10)
            .expect("queue")
            .is_empty()
    );
}

#[test]
fn sqlite_components_complete_in_the_same_transaction() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Sqlite, EmbeddingVectorBackend::Sqlite);
    let retirement_id = seed_retirement(&store, "sqlite-only");
    let mut forbidden = |_: &str| -> Result<(), StoreError> {
        panic!("SQLite projection must not invoke external cleanup")
    };
    for projection in [RetirementProjection::Graph, RetirementProjection::Vector] {
        assert_eq!(
            store
                .attempt_retirement_cleanup(retirement_id, projection, "sqlite", &mut forbidden)
                .expect("already complete"),
            RetirementCleanupOutcome::AlreadyCompleted
        );
    }
    let summary = store.retirement_cleanup_summary(10).expect("summary");
    assert!(summary.available);
    assert_eq!(
        summary.pending_graph
            + summary.pending_vector
            + summary.failed_components
            + summary.legacy_unknown_components,
        0
    );
    assert!(summary.samples.is_empty());
}

#[test]
fn changed_backend_is_explicit_and_never_calls_wrong_store() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "backend-change");
    let mut forbidden =
        |_: &str| -> Result<(), StoreError> { panic!("wrong backend deletion must not run") };
    let error = store
        .attempt_retirement_cleanup(
            retirement_id,
            RetirementProjection::Graph,
            "sqlite",
            &mut forbidden,
        )
        .expect_err("mismatch");
    assert!(error.to_string().contains("backend mismatch"));
    let summary = store.retirement_cleanup_summary(0).expect("summary");
    assert_eq!(
        (
            summary.pending_graph,
            summary.pending_vector,
            summary.failed_components
        ),
        (1, 1, 1)
    );
    assert_eq!(summary.omitted_components, 2);
    assert!(summary.samples.is_empty());
}

#[test]
fn backend_snapshot_is_captured_before_retirement_transaction() {
    let (store, temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let mut config = aether_config::load_workspace_config(temp.path()).expect("config");
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
    save_workspace_config(temp.path(), &config).expect("changed config");
    seed_retirement(&store, "captured");
    let pending = store
        .list_pending_retirement_cleanup(10)
        .expect("captured policy");
    assert_eq!(pending[0].backend, "surreal");
    assert_eq!(pending[1].backend, "lancedb");
}

#[test]
fn reintroduction_supersedes_old_cleanup_without_deleting_current_projection() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "returning");
    let mut current = symbol_record();
    current.id = "returning".to_owned();
    store.upsert_symbol(current).expect("reintroduce");
    let mut forbidden = |_: &str| -> Result<(), StoreError> {
        panic!("old cleanup must not delete new incarnation")
    };
    for (projection, backend) in [
        (RetirementProjection::Graph, "surreal"),
        (RetirementProjection::Vector, "lancedb"),
    ] {
        assert_eq!(
            store
                .attempt_retirement_cleanup(retirement_id, projection, backend, &mut forbidden)
                .expect("guard"),
            RetirementCleanupOutcome::SupersededByReintroduction
        );
    }
    assert!(
        store
            .get_symbol_record("returning")
            .expect("current")
            .is_some()
    );
    assert_eq!(
        store
            .list_symbol_retirements("returning")
            .expect("old archive")
            .len(),
        1
    );
    let summary = store.retirement_cleanup_summary(10).expect("summary");
    assert_eq!(
        (
            summary.pending_graph,
            summary.pending_vector,
            summary.superseded_components
        ),
        (0, 0, 2)
    );
    assert_eq!(
        summary.samples.len(),
        2,
        "superseded is uncertainty, not proof of healthy projections"
    );
}

#[test]
fn cleanup_write_transaction_fences_same_id_publication_across_connections() {
    let (store, temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "fenced");
    let publisher = SqliteStore::open(temp.path()).expect("publisher connection");
    let store = Arc::new(store);
    let cleanup_store = Arc::clone(&store);
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let cleaner = std::thread::spawn(move || {
        cleanup_store.attempt_retirement_cleanup(
            retirement_id,
            RetirementProjection::Graph,
            "surreal",
            &mut |id| {
                assert_eq!(id, "fenced");
                entered_sender
                    .send(())
                    .map_err(|error| StoreError::Graph(error.to_string()))?;
                release_receiver
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| StoreError::Graph(error.to_string()))?;
                Ok(())
            },
        )
    });
    entered_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("cleanup entered");
    let (published_sender, published_receiver) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let mut record = symbol_record();
        record.id = "fenced".to_owned();
        let result = publisher.upsert_symbol(record);
        let _ = published_sender.send(());
        result
    });
    assert!(
        matches!(
            published_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "publication escaped SQLite write fence"
    );
    release_sender.send(()).expect("release callback");
    assert_eq!(
        cleaner.join().expect("cleaner thread").expect("cleanup"),
        RetirementCleanupOutcome::Completed
    );
    writer
        .join()
        .expect("publisher thread")
        .expect("publication after cleanup");
    assert!(
        store
            .get_symbol_record("fenced")
            .expect("current")
            .is_some()
    );
}

#[test]
fn schema22_upgrade_retains_unknown_cleanup_provenance_without_guessing() {
    let (store, temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "legacy");
    let original = store.list_symbol_retirements("legacy").expect("archive");
    store.conn.lock().expect("lock").execute_batch("DROP TABLE retirement_cleanup; PRAGMA user_version = 22; UPDATE schema_version SET version = 22 WHERE component = 'core';").expect("schema22 fixture");
    drop(store);
    let store = SqliteStore::open(temp.path()).expect("upgrade");
    assert!(
        store
            .list_pending_retirement_cleanup(10)
            .expect("no guessed actionable work")
            .is_empty()
    );
    let legacy = store
        .retirement_cleanup_summary(10)
        .expect("legacy provenance");
    assert_eq!(legacy.samples.len(), 2);
    assert!(legacy.samples.iter().all(|record| record.status
        == RetirementCleanupStatus::LegacyUnknown
        && record.backend == "legacy_unknown"));
    let mut forbidden = |_: &str| -> Result<(), StoreError> { panic!("legacy backend is unknown") };
    assert_eq!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut forbidden
            )
            .expect("unknown"),
        RetirementCleanupOutcome::LegacyUnknown
    );
    let summary = store.retirement_cleanup_summary(1).expect("summary");
    assert_eq!(
        (
            summary.pending_graph,
            summary.pending_vector,
            summary.legacy_unknown_components
        ),
        (0, 0, 2)
    );
    assert_eq!(summary.omitted_components, 1);
    assert_eq!(
        store
            .list_symbol_retirements("legacy")
            .expect("retained archive"),
        original
    );
}

#[test]
fn read_only_schema22_reports_queue_unavailable_without_migrating() {
    let (store, temp) = cleanup_fixture(GraphBackend::Sqlite, EmbeddingVectorBackend::Sqlite);
    store.conn.lock().expect("lock").execute_batch("DROP TABLE retirement_cleanup; PRAGMA user_version = 22; UPDATE schema_version SET version = 22 WHERE component = 'core';").expect("schema22 fixture");
    drop(store);
    let store = SqliteStore::open_readonly(temp.path()).expect("read-only schema22");
    assert!(
        !store
            .retirement_cleanup_summary(10)
            .expect("unavailable")
            .available
    );
    assert!(
        store
            .list_pending_retirement_cleanup(10)
            .expect("empty")
            .is_empty()
    );
    assert_eq!(store.get_schema_version().expect("unmigrated").version, 22);
}

#[test]
fn external_delete_before_failed_acknowledgement_is_safe_to_repeat() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let retirement_id = seed_retirement(&store, "at-least-once");
    let present = Cell::new(true);
    let mut interrupted = |_: &str| {
        present.set(false);
        Err(StoreError::Graph(
            "fixture interruption after external deletion".to_owned(),
        ))
    };
    assert!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut interrupted
            )
            .is_err()
    );
    assert!(!present.get());
    assert_eq!(
        store
            .retirement_cleanup_summary(0)
            .expect("pending acknowledgement")
            .pending_graph,
        1
    );
    let mut retry = |_: &str| {
        present.set(false);
        Ok(())
    };
    assert_eq!(
        store
            .attempt_retirement_cleanup(
                retirement_id,
                RetirementProjection::Graph,
                "surreal",
                &mut retry
            )
            .expect("idempotent retry"),
        RetirementCleanupOutcome::Completed
    );
    assert_eq!(
        store
            .retirement_cleanup_summary(0)
            .expect("acknowledged")
            .pending_graph,
        0
    );
}

#[test]
fn cleanup_status_samples_are_bounded_and_report_omissions() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    for index in 0..55 {
        seed_retirement(&store, &format!("bounded-{index}"));
    }
    let summary = store
        .retirement_cleanup_summary(1000)
        .expect("bounded summary");
    assert_eq!((summary.pending_graph, summary.pending_vector), (55, 55));
    assert_eq!(summary.samples.len(), 100);
    assert_eq!(summary.omitted_components, 10);
    assert_eq!(
        store
            .list_pending_retirement_cleanup(1)
            .expect("bounded list")
            .len(),
        1
    );
}

#[test]
fn failed_prefix_cannot_starve_later_jobs_behind_batch_limit() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let first = seed_retirement(&store, "first");
    let second = seed_retirement(&store, "second");
    let third = seed_retirement(&store, "third");
    let mut failure = |_: &str| Err(StoreError::Graph("persistent fixture failure".to_owned()));
    let mut fresh_seen = Vec::new();
    for round in 0..5 {
        let batch = store
            .list_pending_retirement_cleanup(2)
            .expect("next bounded batch");
        assert_eq!(batch.len(), 2);
        if round > 0 {
            assert_eq!(
                batch.iter().filter(|record| record.attempts > 0).count(),
                1,
                "retry quota"
            );
            assert_eq!(
                batch.iter().filter(|record| record.attempts == 0).count(),
                1,
                "fresh quota"
            );
        }
        for record in batch {
            if record.attempts == 0 {
                fresh_seen.push((record.retirement_id, record.projection));
            }
            assert!(
                store
                    .attempt_retirement_cleanup(
                        record.retirement_id,
                        record.projection,
                        &record.backend,
                        &mut failure
                    )
                    .is_err()
            );
        }
    }
    assert_eq!(
        fresh_seen.len(),
        6,
        "every component reached despite permanent failures"
    );
    for id in [first, second, third] {
        assert_eq!(fresh_seen.iter().filter(|(seen, _)| *seen == id).count(), 2);
    }
    let retry = store
        .list_pending_retirement_cleanup(2)
        .expect("unused fresh capacity goes to retries");
    assert_eq!(retry.len(), 2);
    assert!(retry.iter().all(|record| record.attempts > 0));
    let summary = store
        .retirement_cleanup_summary(0)
        .expect("pending failure stats");
    assert_eq!(
        (
            summary.pending_graph,
            summary.pending_vector,
            summary.failed_components
        ),
        (3, 3, 6)
    );
}

#[test]
fn retry_quota_survives_continuous_arrival_of_fresh_work() {
    let (store, _temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Lancedb);
    let old = seed_retirement(&store, "old-retry");
    let mut failure = |_: &str| Err(StoreError::Graph("persistent fixture failure".to_owned()));
    for record in store
        .list_pending_retirement_cleanup(10)
        .expect("initial work")
    {
        assert!(
            store
                .attempt_retirement_cleanup(
                    record.retirement_id,
                    record.projection,
                    &record.backend,
                    &mut failure
                )
                .is_err()
        );
    }
    let mut old_retry_projections = Vec::new();
    for round in 0..8 {
        // Six fresh components arrive per iteration, outpacing the three fresh
        // slots in each batch. A pure attempts-first query never retries old work.
        for fresh in 0..3 {
            seed_retirement(&store, &format!("arrival-{round}-{fresh}"));
        }
        let batch = store
            .list_pending_retirement_cleanup(4)
            .expect("continuous-arrival batch");
        assert_eq!(batch.len(), 4);
        assert_eq!(
            batch.iter().filter(|record| record.attempts > 0).count(),
            1,
            "reserved retry slot"
        );
        assert_eq!(
            batch.iter().filter(|record| record.attempts == 0).count(),
            3,
            "reserved fresh slots"
        );
        for record in batch {
            if record.retirement_id == old {
                old_retry_projections.push(record.projection);
            }
            assert!(
                store
                    .attempt_retirement_cleanup(
                        record.retirement_id,
                        record.projection,
                        &record.backend,
                        &mut failure
                    )
                    .is_err()
            );
        }
    }
    assert!(old_retry_projections.contains(&RetirementProjection::Graph));
    assert!(old_retry_projections.contains(&RetirementProjection::Vector));
}

#[test]
fn projection_filter_precedes_limit_and_preserves_skipped_attempts() {
    let (store, temp) = cleanup_fixture(GraphBackend::Surreal, EmbeddingVectorBackend::Sqlite);
    for index in 0..20 {
        seed_retirement(&store, &format!("graph-only-{index}"));
    }
    let mut config = aether_config::load_workspace_config(temp.path()).expect("config");
    config.storage.graph_backend = GraphBackend::Sqlite;
    config.embeddings.vector_backend = EmbeddingVectorBackend::Lancedb;
    save_workspace_config(temp.path(), &config).expect("new projection policy");
    let vector_store = SqliteStore::open(temp.path()).expect("new policy store");
    let vector_id = seed_retirement(&vector_store, "late-vector");
    let batch = store
        .list_pending_retirement_cleanup_for_projection(RetirementProjection::Vector, 1)
        .expect("vector-only eligibility");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].retirement_id, vector_id);
    assert_eq!(batch[0].projection, RetirementProjection::Vector);
    let graph = store
        .list_pending_retirement_cleanup_for_projection(RetirementProjection::Graph, 100)
        .expect("skipped graph state");
    assert_eq!(graph.len(), 20);
    assert!(graph.iter().all(|record| record.attempts == 0 && record.status == RetirementCleanupStatus::Pending));
}
