use super::*;

#[test]
fn search_project_notes_lexical_matches_query_terms() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_project_note(project_note_record(
            "note-1",
            "We selected sqlite for deterministic local persistence.",
            &["architecture"],
            1_700_000_000,
        ))
        .expect("upsert project note");

    let matches = store
        .search_project_notes_lexical("why sqlite", 10, false, &[])
        .expect("search project notes lexically");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].note_id, "note-1");
}

#[test]
fn coupling_mining_state_round_trip_persists_latest_values() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    assert!(
        store
            .get_coupling_mining_state()
            .expect("read empty state")
            .is_none()
    );

    store
        .upsert_coupling_mining_state(CouplingMiningStateRecord {
            last_commit_hash: Some("abc123".to_owned()),
            last_mined_at: Some(1_700_000_000_000),
            commits_scanned: 42,
        })
        .expect("upsert state");
    store
        .upsert_coupling_mining_state(CouplingMiningStateRecord {
            last_commit_hash: Some("def456".to_owned()),
            last_mined_at: Some(1_700_000_100_000),
            commits_scanned: 99,
        })
        .expect("upsert updated state");

    let state = store
        .get_coupling_mining_state()
        .expect("read state")
        .expect("state exists");
    assert_eq!(state.last_commit_hash.as_deref(), Some("def456"));
    assert_eq!(state.last_mined_at, Some(1_700_000_100_000));
    assert_eq!(state.commits_scanned, 99);
}

#[test]
fn drift_state_and_results_round_trip_and_acknowledge() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    assert!(
        store
            .get_drift_analysis_state()
            .expect("read empty drift state")
            .is_none()
    );

    store
        .upsert_drift_analysis_state(DriftAnalysisStateRecord {
            last_analysis_commit: Some("1111111111111111111111111111111111111111".to_owned()),
            last_analysis_at: Some(1_700_000_000_000),
            symbols_analyzed: 10,
            drift_detected: 2,
        })
        .expect("upsert drift state");

    let state = store
        .get_drift_analysis_state()
        .expect("read drift state")
        .expect("drift state exists");
    assert_eq!(
        state.last_analysis_commit.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(state.symbols_analyzed, 10);
    assert_eq!(state.drift_detected, 2);

    store
        .upsert_drift_results(&[
            DriftResultRecord {
                result_id: "drift-1".to_owned(),
                symbol_id: "sym-a".to_owned(),
                file_path: "src/a.rs".to_owned(),
                symbol_name: "a".to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(0.4),
                current_sir_hash: Some("hash-a2".to_owned()),
                baseline_sir_hash: Some("hash-a1".to_owned()),
                commit_range_start: Some("aaaa".to_owned()),
                commit_range_end: Some("bbbb".to_owned()),
                drift_summary: Some("purpose changed".to_owned()),
                detail_json: "{\"kind\":\"semantic\"}".to_owned(),
                detected_at: 1_700_000_000_100,
                is_acknowledged: false,
            },
            DriftResultRecord {
                result_id: "drift-2".to_owned(),
                symbol_id: "sym-b".to_owned(),
                file_path: "src/b.rs".to_owned(),
                symbol_name: "b".to_owned(),
                drift_type: "boundary_violation".to_owned(),
                drift_magnitude: None,
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: Some("aaaa".to_owned()),
                commit_range_end: Some("bbbb".to_owned()),
                drift_summary: None,
                detail_json: "{\"kind\":\"boundary\"}".to_owned(),
                detected_at: 1_700_000_000_101,
                is_acknowledged: false,
            },
        ])
        .expect("upsert drift results");

    let results = store
        .list_drift_results(false)
        .expect("list unacknowledged results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].result_id, "drift-2");

    let acknowledged = store
        .acknowledge_drift_results(&["drift-1".to_owned()])
        .expect("acknowledge drift result");
    assert_eq!(acknowledged, 1);

    let active = store
        .list_drift_results(false)
        .expect("list filtered results");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].result_id, "drift-2");

    let with_ack = store
        .list_drift_results(true)
        .expect("list all drift results");
    assert_eq!(with_ack.len(), 2);
    assert!(
        with_ack
            .iter()
            .any(|record| record.result_id == "drift-1" && record.is_acknowledged)
    );
}

#[test]
fn community_snapshot_replacement_and_latest_lookup_work() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    assert!(
        store
            .list_latest_community_snapshot()
            .expect("list empty snapshot")
            .is_empty()
    );

    store
        .replace_community_snapshot(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1_700_000_000_000,
            &[
                CommunitySnapshotRecord {
                    snapshot_id: "ignored".to_owned(),
                    symbol_id: "sym-a".to_owned(),
                    community_id: 1,
                    captured_at: 0,
                },
                CommunitySnapshotRecord {
                    snapshot_id: "ignored".to_owned(),
                    symbol_id: "sym-b".to_owned(),
                    community_id: 2,
                    captured_at: 0,
                },
            ],
        )
        .expect("insert first snapshot");
    store
        .replace_community_snapshot(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            1_700_000_000_100,
            &[CommunitySnapshotRecord {
                snapshot_id: "ignored".to_owned(),
                symbol_id: "sym-a".to_owned(),
                community_id: 3,
                captured_at: 0,
            }],
        )
        .expect("insert second snapshot");

    let latest = store
        .list_latest_community_snapshot()
        .expect("list latest community snapshot");
    assert_eq!(latest.len(), 1);
    assert_eq!(
        latest[0].snapshot_id,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert_eq!(latest[0].community_id, 3);
}

#[test]
fn test_intents_round_trip_and_symbol_lookup_work() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .replace_test_intents_for_file(
            "tests/payment_test.rs",
            &[
                test_intent_record(
                    "tests/payment_test.rs",
                    "test_handles_negative_balance",
                    "handles negative balance",
                    Some("sym-test-1"),
                ),
                test_intent_record(
                    "tests/payment_test.rs",
                    "test_returns_none_for_missing_symbol",
                    "returns none for missing symbol",
                    Some("sym-test-2"),
                ),
            ],
        )
        .expect("upsert test intents");

    let file_intents = store
        .list_test_intents_for_file("tests/payment_test.rs")
        .expect("list intents by file");
    assert_eq!(file_intents.len(), 2);
    assert!(
        file_intents
            .iter()
            .any(|record| record.intent_text == "handles negative balance")
    );

    let symbol_intents = store
        .list_test_intents_for_symbol("sym-test-2")
        .expect("list intents by symbol");
    assert_eq!(symbol_intents.len(), 1);
    assert_eq!(
        symbol_intents[0].intent_text,
        "returns none for missing symbol"
    );

    store
        .replace_test_intents_for_file("tests/payment_test.rs", &[])
        .expect("clear intents");
    assert!(
        store
            .list_test_intents_for_file("tests/payment_test.rs")
            .expect("list after clear")
            .is_empty()
    );
}

#[test]
fn search_test_intents_lexical_matches_names_text_and_paths() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .replace_test_intents_for_file(
            "tests/payment_test.rs",
            &[
                test_intent_record(
                    "tests/payment_test.rs",
                    "test_retries_timeout",
                    "retries on timeout",
                    Some("sym-payment"),
                ),
                test_intent_record(
                    "tests/payment_test.rs",
                    "test_records_audit_log",
                    "writes audit trail",
                    Some("sym-payment"),
                ),
            ],
        )
        .expect("upsert payment intents");
    store
        .replace_test_intents_for_file(
            "tests/refund_test.rs",
            &[test_intent_record(
                "tests/refund_test.rs",
                "test_retries_refund_gateway",
                "retries refund gateway failures",
                Some("sym-refund"),
            )],
        )
        .expect("upsert refund intents");

    let retry_hits = store
        .search_test_intents_lexical("retries gateway", 10)
        .expect("search retries");
    assert_eq!(retry_hits.len(), 2);
    assert!(
        retry_hits
            .iter()
            .any(|record| record.file_path == "tests/payment_test.rs")
    );
    assert!(
        retry_hits
            .iter()
            .any(|record| record.file_path == "tests/refund_test.rs")
    );

    let path_hits = store
        .search_test_intents_lexical("payment_test", 10)
        .expect("search by path");
    assert_eq!(path_hits.len(), 2);
}

#[test]
fn list_project_notes_for_file_ref_matches_exact_file_ref() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_project_note(ProjectNoteRecord {
            note_id: "note-a".to_owned(),
            content: "Store contract for graph schema changes".to_owned(),
            content_hash: "hash-a".to_owned(),
            source_type: "manual".to_owned(),
            source_agent: None,
            tags: vec!["architecture".to_owned()],
            entity_refs: Vec::new(),
            file_refs: vec!["crates/aether-store/src/lib.rs".to_owned()],
            symbol_refs: Vec::new(),
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_000_000,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        })
        .expect("upsert matching note");
    store
        .upsert_project_note(ProjectNoteRecord {
            note_id: "note-b".to_owned(),
            content: "Unrelated file".to_owned(),
            content_hash: "hash-b".to_owned(),
            source_type: "manual".to_owned(),
            source_agent: None,
            tags: vec!["misc".to_owned()],
            entity_refs: Vec::new(),
            file_refs: vec!["src/main.rs".to_owned()],
            symbol_refs: Vec::new(),
            created_at: 1_700_000_000_001,
            updated_at: 1_700_000_000_001,
            access_count: 0,
            last_accessed_at: None,
            is_archived: false,
        })
        .expect("upsert non-matching note");

    let matches = store
        .list_project_notes_for_file_ref("crates/aether-store/src/lib.rs", 10)
        .expect("query file ref");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].note_id, "note-a");
}
