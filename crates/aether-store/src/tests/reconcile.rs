use super::*;

#[test]
fn reconcile_migrates_sir_to_new_id() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut old_symbol = symbol_record();
    old_symbol.id = "sym-old".to_owned();
    old_symbol.signature_fingerprint = "sig-old".to_owned();
    let mut new_symbol = old_symbol.clone();
    new_symbol.id = "sym-new".to_owned();
    new_symbol.signature_fingerprint = "sig-new".to_owned();
    new_symbol.last_seen_at += 10;

    store
        .upsert_symbol(old_symbol.clone())
        .expect("upsert old symbol");
    store
        .upsert_symbol(new_symbol.clone())
        .expect("upsert new symbol");
    upsert_sir_state(
        &store,
        old_symbol.id.as_str(),
        "hash-old",
        r#"{"intent":"old"}"#,
        1_700_000_200,
    );
    set_symbol_access(&store, old_symbol.id.as_str(), 2, Some(1_700_000_400));
    set_symbol_access(&store, new_symbol.id.as_str(), 3, Some(1_700_000_300));

    let (migrated, pruned) = store
        .reconcile_and_prune(&[(old_symbol.id.clone(), new_symbol.id.clone())], &[])
        .expect("reconcile old -> new");
    assert_eq!((migrated, pruned), (1, 0));

    assert!(
        store
            .get_symbol_record(old_symbol.id.as_str())
            .expect("query old symbol")
            .is_none()
    );
    assert_eq!(
        store
            .read_sir_blob(new_symbol.id.as_str())
            .expect("read migrated SIR")
            .as_deref(),
        Some(r#"{"intent":"old"}"#)
    );

    let meta = store
        .get_sir_meta(new_symbol.id.as_str())
        .expect("load new SIR meta")
        .expect("new SIR meta exists");
    assert_eq!(meta.sir_hash, "hash-old");

    let history = store
        .list_sir_history(new_symbol.id.as_str())
        .expect("list migrated history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sir_hash, "hash-old");

    let search = store
        .get_symbol_search_result(new_symbol.id.as_str())
        .expect("load search result")
        .expect("search result exists");
    assert_eq!(search.access_count, 5);
    assert_eq!(search.last_accessed_at, Some(1_700_000_400));
}

#[test]
fn prune_removes_orphans_not_in_snapshot() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut orphan = symbol_record();
    orphan.id = "sym-orphan".to_owned();
    let mut target = symbol_record_ts();
    target.id = "sym-target".to_owned();
    target.qualified_name = "demo::target".to_owned();

    store.upsert_symbol(orphan.clone()).expect("upsert orphan");
    store.upsert_symbol(target.clone()).expect("upsert target");
    upsert_sir_state(
        &store,
        orphan.id.as_str(),
        "hash-orphan",
        r#"{"intent":"orphan"}"#,
        1_700_000_200,
    );
    store
        .upsert_edges(&[calls_edge(
            orphan.id.as_str(),
            target.qualified_name.as_str(),
            orphan.file_path.as_str(),
        )])
        .expect("upsert orphan edge");
    store
        .enqueue_sir_request(orphan.id.as_str())
        .expect("enqueue sir request");

    let mut intent = write_intent_record("intent-orphan", WriteIntentStatus::Pending);
    intent.symbol_id = orphan.id.clone();
    store.create_write_intent(&intent).expect("create intent");

    let (migrated, pruned) = store
        .reconcile_and_prune(&[], &[orphan.id.clone()])
        .expect("prune orphan");
    assert_eq!((migrated, pruned), (0, 1));

    assert!(
        store
            .get_symbol_record(orphan.id.as_str())
            .expect("query orphan")
            .is_none()
    );
    assert!(
        store
            .get_sir_meta(orphan.id.as_str())
            .expect("query orphan meta")
            .is_none()
    );
    assert!(
        store
            .list_sir_history(orphan.id.as_str())
            .expect("query orphan history")
            .is_empty()
    );
    assert!(
        store
            .list_sir_request_symbol_ids(10)
            .expect("list pending requests")
            .is_empty()
    );
    assert!(
        store
            .get_incomplete_intents()
            .expect("list incomplete intents")
            .is_empty()
    );

    let edge_count: i64 = store
        .conn
        .lock()
        .expect("lock store connection")
        .query_row(
            "SELECT COUNT(*) FROM symbol_edges WHERE source_id = ?1",
            params![orphan.id],
            |row| row.get(0),
        )
        .expect("count orphan edges");
    assert_eq!(edge_count, 0);
}

#[test]
fn reconcile_preserves_sir_history() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut old_symbol = symbol_record();
    old_symbol.id = "sym-history-old".to_owned();
    let mut new_symbol = old_symbol.clone();
    new_symbol.id = "sym-history-new".to_owned();
    new_symbol.signature_fingerprint = "sig-history-new".to_owned();

    store.upsert_symbol(old_symbol.clone()).expect("upsert old");
    store.upsert_symbol(new_symbol.clone()).expect("upsert new");
    upsert_sir_state(
        &store,
        old_symbol.id.as_str(),
        "hash-v1",
        r#"{"intent":"v1"}"#,
        1_700_000_100,
    );
    upsert_sir_state(
        &store,
        old_symbol.id.as_str(),
        "hash-v2",
        r#"{"intent":"v2"}"#,
        1_700_000_200,
    );

    store
        .reconcile_and_prune(&[(old_symbol.id.clone(), new_symbol.id.clone())], &[])
        .expect("reconcile history");

    let history = store
        .list_sir_history(new_symbol.id.as_str())
        .expect("list new history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version, 1);
    assert_eq!(history[0].sir_hash, "hash-v1");
    assert_eq!(history[1].version, 2);
    assert_eq!(history[1].sir_hash, "hash-v2");
}

#[test]
fn reconcile_handles_new_id_already_has_sir() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut old_symbol = symbol_record();
    old_symbol.id = "sym-conflict-old".to_owned();
    let mut new_symbol = old_symbol.clone();
    new_symbol.id = "sym-conflict-new".to_owned();
    new_symbol.signature_fingerprint = "sig-conflict-new".to_owned();

    store.upsert_symbol(old_symbol.clone()).expect("upsert old");
    store.upsert_symbol(new_symbol.clone()).expect("upsert new");
    upsert_sir_state(
        &store,
        old_symbol.id.as_str(),
        "hash-old",
        r#"{"intent":"old"}"#,
        1_700_000_100,
    );
    upsert_sir_state(
        &store,
        new_symbol.id.as_str(),
        "hash-new",
        r#"{"intent":"new"}"#,
        1_700_000_300,
    );

    let (migrated, pruned) = store
        .reconcile_and_prune(&[(old_symbol.id.clone(), new_symbol.id.clone())], &[])
        .expect("reconcile conflicting SIR");
    assert_eq!((migrated, pruned), (1, 0));

    assert_eq!(
        store
            .read_sir_blob(new_symbol.id.as_str())
            .expect("read winning SIR")
            .as_deref(),
        Some(r#"{"intent":"new"}"#)
    );
    let history = store
        .list_sir_history(new_symbol.id.as_str())
        .expect("list retained history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sir_hash, "hash-new");
    assert!(
        store
            .list_sir_history(old_symbol.id.as_str())
            .expect("query stale history")
            .is_empty()
    );
}
