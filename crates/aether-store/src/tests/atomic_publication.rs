use super::*;

fn seeded_store(workspace: &std::path::Path) -> (SqliteStore, SymbolRecord) {
    let store = SqliteStore::open(workspace).expect("store");
    seed_live_symbol_cleanup_state(&store, "old");
    store
        .conn
        .lock()
        .expect("connection")
        .execute(
            "UPDATE sir SET source_hash='original-source' WHERE id='old'",
            [],
        )
        .expect("source hash");
    let mut replacement = store
        .get_symbol_record("old")
        .expect("record")
        .expect("old");
    replacement.id = "new".to_owned();
    replacement.signature_fingerprint = "replacement-signature".to_owned();
    (store, replacement)
}

fn publish(store: &SqliteStore, replacement: SymbolRecord) -> Result<(usize, usize), StoreError> {
    store.publish_and_reconcile_symbols(
        &[replacement],
        &[("old".to_owned(), "new".to_owned())],
        &[],
    )
}

#[test]
fn failed_retirement_rolls_back_replacement_publication() {
    let temp = tempdir().expect("tempdir");
    let (store, replacement) = seeded_store(temp.path());
    store.conn.lock().expect("connection").execute_batch(
        "CREATE TRIGGER fail_retirement BEFORE DELETE ON symbols WHEN OLD.id='old' BEGIN SELECT RAISE(ABORT,'fixture interruption'); END;"
    ).expect("trigger");
    assert!(publish(&store, replacement.clone()).is_err());
    drop(store);
    let store = SqliteStore::open(temp.path()).expect("restart");
    assert!(
        store.get_symbol_record("new").expect("record").is_none(),
        "replacement must roll back with failed retirement"
    );
    assert!(store.get_symbol_record("old").expect("record").is_some());
    assert!(
        store
            .list_symbol_retirements("old")
            .expect("retirements")
            .is_empty()
    );
    assert!(
        store
            .list_symbol_lineage("new")
            .expect("lineage")
            .is_empty()
    );
    assert_eq!(
        store
            .list_active_contracts_for_symbol("old")
            .expect("contracts")
            .len(),
        1
    );
    store
        .conn
        .lock()
        .expect("connection")
        .execute_batch("DROP TRIGGER fail_retirement;")
        .expect("recover");
    assert_eq!(publish(&store, replacement.clone()).expect("retry"), (1, 0));
    assert_eq!(publish(&store, replacement).expect("repeat"), (0, 0));
    assert_eq!(store.list_symbol_lineage("new").expect("lineage").len(), 1);
}

#[test]
fn leftover_retired_mirror_cannot_rehydrate_active_evidence() {
    let temp = tempdir().expect("tempdir");
    let (store, replacement) = seeded_store(temp.path());
    let payload = store.read_sir_blob("old").expect("payload").expect("old");
    publish(&store, replacement).expect("publish");
    // Exact post-commit/pre-cleanup disk state: canonical retirement committed,
    // old mirror survived. The separately tested child crash exercises this boundary.
    fs::write(store.sir_dir().join("old.json"), &payload).expect("leftover mirror");
    drop(store);
    let store = SqliteStore::open(temp.path()).expect("restart");
    assert!(
        store.read_sir_blob("old").expect("read").is_none(),
        "retired mirror must not recreate active SIR"
    );
    assert!(store.get_sir_meta("old").expect("metadata").is_none());
    assert_eq!(
        store.read_sir_blob("new").expect("new").as_deref(),
        Some(payload.as_str())
    );
    let readonly = SqliteStore::open_readonly(temp.path()).expect("read-only restart");
    assert!(
        readonly
            .read_sir_blob("old")
            .expect("read-only tombstone")
            .is_none()
    );
    drop(readonly);
    let mut returning = symbol_record();
    returning.id = "old".to_owned();
    store
        .upsert_symbol(returning)
        .expect("reintroduced identity");
    assert!(
        store
            .read_sir_blob("old")
            .expect("old incarnation")
            .is_none()
    );
    store
        .write_sir_blob("old", r#"{"intent":"explicit new incarnation"}"#)
        .expect("new write");
    assert!(
        store
            .read_sir_blob("old")
            .expect("new incarnation")
            .expect("payload")
            .contains("explicit new incarnation")
    );
}

#[test]
fn invalid_and_unreadable_publications_have_no_partial_effects() {
    let temp = tempdir().expect("tempdir");
    let (store, replacement) = seeded_store(temp.path());
    for (migrations, prunes) in [
        (vec![("old".to_owned(), "missing".to_owned())], vec![]),
        (
            vec![("old".to_owned(), "new".to_owned())],
            vec!["new".to_owned()],
        ),
    ] {
        assert!(
            store
                .publish_and_reconcile_symbols(
                    std::slice::from_ref(&replacement),
                    &migrations,
                    &prunes
                )
                .is_err()
        );
        assert!(store.get_symbol_record("new").expect("record").is_none());
        assert!(
            store
                .list_symbol_retirements("old")
                .expect("archive")
                .is_empty()
        );
    }
    // Force retirement to consult its only legacy payload, which cannot be decoded.
    store.conn.lock().expect("connection").execute_batch(
        "UPDATE sir SET sir_json=NULL WHERE id='old'; DELETE FROM sir_history WHERE symbol_id='old';"
    ).expect("legacy");
    fs::write(store.sir_dir().join("old.json"), [0xff, 0xfe]).expect("unreadable payload");
    assert!(publish(&store, replacement).is_err());
    assert!(store.get_symbol_record("new").expect("record").is_none());
    assert!(
        store
            .list_symbol_retirements("old")
            .expect("archive")
            .is_empty()
    );
    assert_eq!(
        fs::read(store.sir_dir().join("old.json")).expect("unchanged mirror"),
        [0xff, 0xfe]
    );
}

#[test]
fn record_only_empty_and_sql_constraint_failure_are_atomic() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("store");
    assert_eq!(
        store
            .publish_and_reconcile_symbols(&[], &[], &[])
            .expect("empty"),
        (0, 0)
    );
    let record = symbol_record();
    assert_eq!(
        store
            .publish_and_reconcile_symbols(std::slice::from_ref(&record), &[], &[])
            .expect("record only"),
        (0, 0)
    );
    store.conn.lock().expect("connection").execute_batch(
        "CREATE TRIGGER fail_second BEFORE INSERT ON symbols WHEN NEW.id='second' BEGIN SELECT RAISE(ABORT,'second insert'); END;"
    ).expect("trigger");
    let mut first = record.clone();
    first.id = "first".to_owned();
    let mut second = record;
    second.id = "second".to_owned();
    assert!(
        store
            .publish_and_reconcile_symbols(&[first, second], &[], &[])
            .is_err()
    );
    assert!(store.get_symbol_record("first").expect("record").is_none());
}

#[test]
fn crash_child() {
    let Some(workspace) = std::env::var_os("AETHER_PUBLICATION_CRASH_FIXTURE") else {
        return;
    };
    let mode = std::env::var("AETHER_PUBLICATION_CRASH_MODE").expect("mode");
    let store = SqliteStore::open(std::path::Path::new(&workspace)).expect("child store");
    let mut replacement = store.get_symbol_record("old").expect("read").expect("old");
    replacement.id = "new".to_owned();
    replacement.signature_fingerprint = "replacement-signature".to_owned();
    let conn = store.conn.lock().expect("connection");
    let tx =
        Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).expect("transaction");
    if mode == "before_commit" {
        // Exit at the exact former gap, with replacement insertion still uncommitted.
        crate::symbols::upsert_symbols_tx(&tx, &[replacement]).expect("replacement");
        std::process::exit(81);
    }
    store
        .reconcile_symbols_tx(
            &tx,
            &[replacement],
            &[("old".to_owned(), "new".to_owned())],
            &[],
            true,
        )
        .expect("canonical transaction");
    if mode == "before_retirement_commit" {
        std::process::exit(83);
    }
    tx.commit().expect("commit");
    // No destructor or post-commit mirror cleanup gets to run.
    std::process::exit(82);
}

#[test]
fn real_process_exit_before_commit_and_before_mirrors_recovers_once() {
    for (mode, code) in [
        ("before_commit", 81),
        ("before_retirement_commit", 83),
        ("after_commit", 82),
    ] {
        let temp = tempdir().expect("tempdir");
        let (store, replacement) = seeded_store(temp.path());
        let payload = store.read_sir_blob("old").expect("read").expect("old");
        fs::write(store.sir_dir().join("old.json"), &payload).expect("legacy mirror");
        drop(store);
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "tests::atomic_publication::crash_child",
                "--nocapture",
            ])
            .env("AETHER_PUBLICATION_CRASH_FIXTURE", temp.path())
            .env("AETHER_PUBLICATION_CRASH_MODE", mode)
            .status()
            .expect("child");
        assert_eq!(status.code(), Some(code));
        let store = SqliteStore::open(temp.path()).expect("restart");
        assert!(
            store.sir_dir().join("old.json").exists(),
            "child must exit before mirrors"
        );
        if mode != "after_commit" {
            assert!(store.get_symbol_record("new").expect("new").is_none());
            assert_eq!(
                store.read_sir_blob("old").expect("old").as_deref(),
                Some(payload.as_str())
            );
            assert!(
                store
                    .list_symbol_retirements("old")
                    .expect("archive")
                    .is_empty()
            );
            assert!(
                store
                    .list_symbol_lineage("new")
                    .expect("lineage")
                    .is_empty()
            );
            assert_eq!(publish(&store, replacement.clone()).expect("retry"), (1, 0));
        } else {
            assert!(store.get_symbol_record("old").expect("old").is_none());
            assert!(store.read_sir_blob("old").expect("tombstone").is_none());
        }
        assert_eq!(
            publish(&store, replacement).expect("idempotent restart"),
            (0, 0)
        );
        assert_eq!(
            store.read_sir_blob("new").expect("new").as_deref(),
            Some(payload.as_str())
        );
        assert_eq!(
            store.get_sir_source_hash("new").expect("source").as_deref(),
            Some("original-source")
        );
        assert_eq!(
            store
                .get_sir_meta("new")
                .expect("meta")
                .expect("new")
                .sir_status,
            "stale"
        );
        assert_eq!(store.list_symbol_lineage("new").expect("lineage").len(), 1);
        assert_eq!(
            store.list_symbol_retirements("old").expect("archive").len(),
            1
        );
        assert_eq!(
            store
                .list_active_contracts_for_symbol("new")
                .expect("contracts")
                .len(),
            1
        );
        assert_eq!(store.list_sir_history("new").expect("history").len(), 1);
        let conn = store.conn.lock().expect("connection");
        let obligations: i64 = conn
            .query_row("SELECT count(*) FROM retirement_cleanup", [], |row| {
                row.get(0)
            })
            .expect("outbox");
        assert!(
            obligations > 0,
            "cleanup obligation must commit with retirement"
        );
    }
}

#[test]
fn concurrent_replays_commit_one_lineage_and_retirement() {
    let temp = tempdir().expect("tempdir");
    let (store, replacement) = seeded_store(temp.path());
    drop(store);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = temp.path().to_path_buf();
        let barrier = barrier.clone();
        let replacement = replacement.clone();
        handles.push(std::thread::spawn(move || {
            let store = SqliteStore::open(path).expect("independent connection");
            barrier.wait();
            publish(&store, replacement).expect("serialized publication")
        }));
    }
    let mut counts = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer"))
        .collect::<Vec<_>>();
    counts.sort();
    assert_eq!(counts, [(0, 0), (1, 0)]);
    let store = SqliteStore::open(temp.path()).expect("restart");
    assert_eq!(store.list_symbol_lineage("new").expect("lineage").len(), 1);
    assert_eq!(
        store
            .list_symbol_retirements("old")
            .expect("retirement")
            .len(),
        1
    );
}
