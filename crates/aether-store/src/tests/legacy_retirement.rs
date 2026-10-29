use super::*;

const PAYLOAD: &str = r#"{"intent":"retain the only legacy payload"}"#;

fn legacy(store: &SqliteStore, id: &str, with_meta: bool) {
    let mut symbol = symbol_record();
    symbol.id = id.to_owned();
    store.upsert_symbol(symbol).expect("symbol");
    if with_meta {
        let mut meta = sir_meta_record();
        meta.id = id.to_owned();
        store.upsert_sir_meta(meta).expect("legacy metadata");
    }
    fs::write(store.sir_dir().join(format!("{id}.json")), PAYLOAD).expect("mirror");
}

fn assert_retained(store: &SqliteStore, id: &str) {
    let archives = store.list_symbol_retirements(id).expect("archives");
    assert_eq!(archives.len(), 1);
    assert_eq!(
        archives[0].evidence["legacy_sir_blob"][0]["sir_json"],
        PAYLOAD
    );
    assert!(!store.sir_dir().join(format!("{id}.json")).exists());
}

#[test]
fn legacy_payload_survives_direct_removal_and_prune() {
    for (prune, with_meta) in [(false, true), (true, true), (false, false), (true, false)] {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open");
        legacy(&store, "old", with_meta);
        if prune {
            store
                .reconcile_and_prune(&[], &["old".to_owned()])
                .expect("prune");
        } else {
            store.mark_removed("old").expect("remove");
        }
        assert_retained(&store, "old");
    }
}

#[test]
fn legacy_payload_migrates_without_losing_old_attribution() {
    for newer_target in [false, true] {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open");
        legacy(&store, "old", true);
        let mut target = symbol_record();
        target.id = "new".to_owned();
        target.signature_fingerprint = "new-signature".to_owned();
        store.upsert_symbol(target).expect("target");
        if newer_target {
            let mut meta = sir_meta_record();
            meta.id = "new".to_owned();
            meta.updated_at += 100;
            store.upsert_sir_meta(meta).expect("newer target metadata");
            store
                .write_sir_blob("new", r#"{"intent":"newer target"}"#)
                .expect("new target");
        }
        store
            .reconcile_and_prune(&[("old".to_owned(), "new".to_owned())], &[])
            .expect("migrate");
        assert_retained(&store, "old");
        assert_eq!(
            store
                .read_sir_blob("new")
                .expect("target payload")
                .as_deref(),
            Some(if newer_target {
                r#"{"intent":"newer target"}"#
            } else {
                PAYLOAD
            })
        );
    }
}

#[test]
fn sqlite_payload_remains_authoritative_over_stale_mirror() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open");
    legacy(&store, "old", true);
    store
        .write_sir_blob("old", r#"{"intent":"canonical"}"#)
        .expect("canonical");
    fs::write(store.sir_dir().join("old.json"), PAYLOAD).expect("stale mirror");
    store.mark_removed("old").expect("remove");
    let archives = store.list_symbol_retirements("old").expect("archives");
    assert_eq!(
        archives[0].evidence["sir"][0]["sir_json"],
        r#"{"intent":"canonical"}"#
    );
    assert!(!archives[0].evidence.contains_key("legacy_sir_blob"));
}

#[test]
fn unreadable_legacy_payload_refuses_retirement_and_keeps_originals() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open");
    legacy(&store, "old", true);
    let path = store.sir_dir().join("old.json");
    fs::write(&path, [0xff, 0xfe]).expect("invalid utf8");
    assert!(store.mark_removed("old").is_err());
    assert!(store.get_symbol_record("old").expect("symbol").is_some());
    assert!(
        store
            .list_symbol_retirements("old")
            .expect("archives")
            .is_empty()
    );
    assert_eq!(fs::read(path).expect("original mirror"), [0xff, 0xfe]);
}

#[test]
fn failed_retirement_rolls_back_payload_hydration() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open");
    legacy(&store, "old", true);
    store.conn.lock().expect("lock").execute_batch(
        "CREATE TRIGGER fail_retirement BEFORE DELETE ON symbols BEGIN SELECT RAISE(ABORT,'fixture rollback'); END;"
    ).expect("failure injection");
    assert!(store.mark_removed("old").is_err());
    assert!(store.get_symbol_record("old").expect("symbol").is_some());
    assert!(
        store
            .list_symbol_retirements("old")
            .expect("archives")
            .is_empty()
    );
    let json: Option<String> = store
        .conn
        .lock()
        .expect("lock")
        .query_row("SELECT sir_json FROM sir WHERE id='old'", [], |row| {
            row.get(0)
        })
        .expect("original database payload");
    assert!(json.is_none(), "hydration must roll back with retirement");
    assert_eq!(
        fs::read_to_string(store.sir_dir().join("old.json")).expect("mirror"),
        PAYLOAD
    );
}
