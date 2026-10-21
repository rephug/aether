use super::*;

#[test]
fn removal_archives_original_evidence_and_is_idempotent() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_live_symbol_cleanup_state(&store, "retired");
    store.mark_removed("retired").expect("retire");
    store.mark_removed("retired").expect("repeat retirement");
    assert_live_symbol_cleanup_empty(&store, "retired");
    let archives = store.list_symbol_retirements("retired").expect("archive");
    assert_eq!(archives.len(), 1);
    let archive = &archives[0];
    assert_eq!(archive.reason, "removed");
    assert_eq!(archive.symbol.as_ref().expect("metadata").id, "retired");
    for table in [
        "sir",
        "sir_history",
        "sir_fingerprint_history",
        "intent_contracts",
        "intent_violations",
        "test_intents",
        "drift_results",
        "sir_quality",
        "sir_audit",
        "symbol_edges",
        "community_snapshot",
    ] {
        assert_eq!(archive.evidence[table].len(), 1, "lost {table}");
    }
    assert_eq!(
        archive.evidence["intent_contracts"][0]["clause_text"],
        "Preserve cleanup semantics"
    );
    assert_eq!(
        archive.evidence["intent_violations"][0]["symbol_id"],
        "retired"
    );
    assert_eq!(
        archive.evidence["sir_history"][0]["sir_json"],
        r#"{"intent":"cleanup retired"}"#
    );
}

#[test]
fn retained_orphan_audit_rows_do_not_create_repeated_retirements() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let conn = store.conn.lock().expect("lock");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("legacy orphan fixture");
    conn.execute("INSERT INTO sir_audit (symbol_id, audit_type, severity, category, certainty, trigger_condition, impact, description, related_symbols, model, provider, status, created_at) VALUES ('orphan-audit', 'symbol', 'high', 'test', 'confirmed', 'test', 'test', 'test', '[]', 'mock', 'mock', 'open', 1)", []).expect("orphan audit");
    drop(conn);
    store.mark_removed("orphan-audit").expect("first no-op");
    store.mark_removed("orphan-audit").expect("second no-op");
    assert!(
        store
            .list_symbol_retirements("orphan-audit")
            .expect("archives")
            .is_empty()
    );
    assert_eq!(
        count_symbol_rows(
            &store,
            "SELECT COUNT(*) FROM sir_audit WHERE symbol_id = ?1",
            "orphan-audit"
        ),
        1
    );
}

#[test]
fn same_id_reintroduction_starts_without_inheriting_retired_evidence() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_live_symbol_cleanup_state(&store, "returning");
    let symbol = store
        .get_symbol_record("returning")
        .expect("read")
        .expect("symbol");
    store.mark_removed("returning").expect("retire");
    store.upsert_symbol(symbol).expect("reintroduce");
    assert!(store.get_sir_meta("returning").expect("meta").is_none());
    assert!(
        store
            .list_active_contracts_for_symbol("returning")
            .expect("contracts")
            .is_empty()
    );
    assert!(
        store
            .list_sir_history("returning")
            .expect("active history")
            .is_empty()
    );
    upsert_sir_state(
        &store,
        "returning",
        "new-hash",
        r#"{"intent":"different incarnation"}"#,
        1_800_000_000,
    );
    store.mark_removed("returning").expect("retire again");
    let archives = store
        .list_symbol_retirements("returning")
        .expect("archives");
    assert_eq!(archives.len(), 2);
    assert_ne!(archives[0].retirement_id, archives[1].retirement_id);
    assert_eq!(archives[0].evidence["sir_history"][0]["version"], 1);
    assert_eq!(archives[1].evidence["sir_history"][0]["version"], 1);
    assert!(archives[1].evidence["intent_contracts"].is_empty());
}

#[test]
fn migration_archives_before_contract_moves_and_retains_conflicting_history() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_live_symbol_cleanup_state(&store, "old");
    let mut target = store.get_symbol_record("old").expect("read").expect("old");
    target.id = "new".to_owned();
    target.signature_fingerprint = "changed-signature".to_owned();
    store.upsert_symbol(target).expect("new");
    upsert_sir_state(
        &store,
        "new",
        "newest-hash",
        r#"{"intent":"target wins"}"#,
        1_900_000_000,
    );
    store
        .reconcile_and_prune(&[("old".to_owned(), "new".to_owned())], &[])
        .expect("migrate");
    let archives = store.list_symbol_retirements("old").expect("old evidence");
    assert_eq!(archives.len(), 1);
    assert_eq!(
        archives[0].evidence["intent_contracts"][0]["symbol_id"],
        "old"
    );
    assert_eq!(
        archives[0].evidence["intent_violations"][0]["symbol_id"],
        "old"
    );
    assert_eq!(
        archives[0].evidence["sir_history"][0]["sir_hash"],
        "hash-old"
    );
    assert_eq!(
        store.list_sir_history("new").expect("target history")[0].sir_hash,
        "newest-hash"
    );
    assert_eq!(
        store
            .list_active_contracts_for_symbol("new")
            .expect("contracts")
            .len(),
        1
    );
    let lineage = store.list_symbol_lineage("new").expect("lineage");
    assert_eq!(lineage.len(), 1);
    assert_eq!(lineage[0].old_symbol_id, "old");
    assert_eq!(lineage[0].retirement_id, archives[0].retirement_id);
    assert_eq!(lineage[0].confidence, "heuristic");
    assert!(lineage[0].source_revision.is_none());
    assert_eq!(
        store.list_symbol_lineage("old").expect("reverse lookup"),
        lineage
    );
}

#[test]
fn failed_retirement_rolls_back_archive_and_active_deletion_together() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_live_symbol_cleanup_state(&store, "blocked");
    store.conn.lock().expect("lock").execute_batch("CREATE TRIGGER reject_symbol_delete BEFORE DELETE ON symbols BEGIN SELECT RAISE(ABORT, 'injected deletion failure'); END;").expect("trigger");
    assert!(store.mark_removed("blocked").is_err());
    assert!(
        store
            .list_symbol_retirements("blocked")
            .expect("archives")
            .is_empty()
    );
    assert!(
        store
            .get_symbol_record("blocked")
            .expect("symbol")
            .is_some()
    );
    assert_eq!(
        store
            .list_active_contracts_for_symbol("blocked")
            .expect("contracts")
            .len(),
        1
    );
    assert_eq!(store.list_sir_history("blocked").expect("history").len(), 1);
}

#[test]
fn lineage_rejects_renames_and_many_to_one_guesses() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let old = symbol_record();
    let mut new = old.clone();
    new.id = "new".to_owned();
    new.qualified_name = "renamed".to_owned();
    new.signature_fingerprint = "new".to_owned();
    let mut other = old.clone();
    other.id = "other".to_owned();
    for symbol in [old.clone(), new, other] {
        store.upsert_symbol(symbol).expect("upsert");
    }
    assert!(
        store
            .reconcile_and_prune(&[(old.id.clone(), "new".to_owned())], &[])
            .is_err()
    );
    assert!(
        store
            .reconcile_and_prune(
                &[
                    (old.id.clone(), "new".to_owned()),
                    ("other".to_owned(), "new".to_owned())
                ],
                &[]
            )
            .is_err()
    );
    assert!(
        store
            .list_symbol_retirements(&old.id)
            .expect("archive")
            .is_empty()
    );
    assert!(store.get_symbol_record(&old.id).expect("symbol").is_some());
}

#[test]
fn schema21_upgrade_retains_existing_evidence_and_adds_retirement_tables() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_live_symbol_cleanup_state(&store, "upgrade");
    store.conn.lock().expect("lock").execute_batch("DROP TABLE symbol_lineage; DROP TABLE symbol_retirements; PRAGMA user_version = 21; UPDATE schema_version SET version = 21 WHERE component = 'core';").expect("schema21 fixture");
    drop(store);
    let store = SqliteStore::open(temp.path()).expect("upgrade fixture");
    assert_eq!(
        store.get_schema_version().expect("version").version,
        CORE_SCHEMA_VERSION
    );
    assert_eq!(store.list_sir_history("upgrade").expect("history").len(), 1);
    store.mark_removed("upgrade").expect("retire after upgrade");
    assert_eq!(
        store
            .list_symbol_retirements("upgrade")
            .expect("archive")
            .len(),
        1
    );
}

#[test]
fn retirement_discovery_reads_bounded_metadata_without_loading_archived_payloads() {
    let temp = tempdir().expect("workspace");
    let store = SqliteStore::open(temp.path()).expect("store");
    let mut symbol = symbol_record();
    symbol.id = "old-bridge".to_owned();
    symbol.qualified_name = "legacy_bridge".to_owned();
    symbol.file_path = "src/old_bridge.ts".to_owned();
    store
        .upsert_symbol(symbol.clone())
        .expect("old declaration");
    store.mark_removed(&symbol.id).expect("retire");
    // Payload corruption must not turn a small discovery query into a payload read.
    store.conn.lock().expect("lock").execute(
        "UPDATE symbol_retirements SET evidence_json = 'not valid archive JSON' WHERE symbol_id = ?1",
        [&symbol.id],
    ).expect("payload isolation fixture");
    let found = store
        .search_symbol_retirement_summaries(&["old_bridge".to_owned()], 1)
        .expect("metadata search");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].symbol, symbol);
    assert!(
        store.list_symbol_retirements(&symbol.id).is_err(),
        "explicit payload lookup should observe corruption"
    );
    assert!(
        store
            .search_symbol_retirement_summaries(&["%".to_owned()], 10)
            .expect("literal wildcard")
            .is_empty()
    );
    assert!(
        store
            .search_symbol_retirement_summaries(&[], 10)
            .expect("empty terms")
            .is_empty()
    );
    assert!(
        store
            .search_symbol_retirement_summaries(&["bridge".to_owned()], 0)
            .expect("zero limit")
            .is_empty()
    );
    let connection = store.conn.lock().expect("lock");
    for index in 0..105 {
        connection.execute(
            "INSERT INTO symbol_retirements (symbol_id, retired_at, reason, symbol_json, evidence_json) VALUES (?1, 1, 'fixture', ?2, 'unread payload')",
            params![format!("retirement-{index}"), serde_json::to_string(&symbol).expect("small metadata")],
        ).expect("many metadata rows");
    }
    drop(connection);
    assert_eq!(
        store
            .search_symbol_retirement_summaries(&["bridge".to_owned()], u32::MAX)
            .expect("bounded search")
            .len(),
        100
    );
}
