use super::*;

#[test]
fn recent_history_metadata_is_bounded_and_never_selects_payload_json() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("store");
    for version in 1..=105 {
        let payload = if version == 105 {
            "{".repeat(1024 * 1024)
        } else {
            "not valid JSON".to_owned()
        };
        store
            .record_sir_version_if_changed(
                "history-id",
                &format!("hash-{version}"),
                "saved-provider",
                "saved-model",
                &payload,
                version,
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            )
            .expect("saved malformed fixture payload");
    }
    let conn = store.conn.lock().expect("lock");
    conn.execute_batch(
        "ALTER TABLE sir_history RENAME TO saved_history_payloads;
         CREATE VIEW sir_history AS SELECT symbol_id, version, sir_hash, provider,
             model, created_at, commit_hash, json(sir_json) AS sir_json
         FROM saved_history_payloads;",
    )
    .expect("payload access poison view");
    drop(conn);
    // Selecting this view's sir_json invokes json() on malformed/large input and
    // fails. Success below therefore proves the metadata query omits that column.
    assert!(store.list_sir_history("history-id").is_err());
    let recent = store
        .list_recent_sir_history_metadata("history-id", 2)
        .expect("metadata only");
    assert_eq!(
        recent
            .iter()
            .map(|record| record.version)
            .collect::<Vec<_>>(),
        vec![105, 104]
    );
    assert_eq!(recent[0].sir_hash, "hash-105");
    assert_eq!(recent[0].provider, "saved-provider");
    assert_eq!(
        recent[0].commit_hash.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(
        store
            .list_recent_sir_history_metadata("history-id", 0)
            .expect("zero limit")
            .is_empty()
    );
    assert!(
        store
            .list_recent_sir_history_metadata("missing-id", 2)
            .expect("missing")
            .is_empty()
    );
    assert_eq!(
        store
            .list_recent_sir_history_metadata("history-id", 1000)
            .expect("cap")
            .len(),
        100
    );
    assert!(
        !serde_json::to_string(&recent)
            .expect("metadata serialization")
            .contains("sir_json")
    );
}
