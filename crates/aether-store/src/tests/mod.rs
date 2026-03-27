use std::fs;

use tempfile::tempdir;

use super::*;

fn symbol_record() -> SymbolRecord {
    SymbolRecord {
        id: "sym-1".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "demo::run".to_owned(),
        signature_fingerprint: "sig-a".to_owned(),
        last_seen_at: 1_700_000_000,
    }
}

fn sir_meta_record() -> SirMetaRecord {
    SirMetaRecord {
        id: "sym-1".to_owned(),
        sir_hash: "hash-a".to_owned(),
        sir_version: 1,
        provider: "none".to_owned(),
        model: "none".to_owned(),
        generation_pass: "scan".to_owned(),
        reasoning_trace: None,
        prompt_hash: None,
        staleness_score: None,
        updated_at: 1_700_000_100,
        sir_status: "fresh".to_owned(),
        last_error: None,
        last_attempt_at: 1_700_000_100,
    }
}

fn symbol_record_ts() -> SymbolRecord {
    SymbolRecord {
        id: "sym-2".to_owned(),
        file_path: "src/app.ts".to_owned(),
        language: "typescript".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "web::render".to_owned(),
        signature_fingerprint: "sig-c".to_owned(),
        last_seen_at: 1_700_000_000,
    }
}

fn embedding_record(symbol_id: &str, sir_hash: &str, embedding: Vec<f32>) -> SymbolEmbeddingRecord {
    SymbolEmbeddingRecord {
        symbol_id: symbol_id.to_owned(),
        sir_hash: sir_hash.to_owned(),
        provider: "mock".to_owned(),
        model: "mock-64d".to_owned(),
        embedding,
        updated_at: 1_700_000_500,
    }
}

fn upsert_sir_state(
    store: &SqliteStore,
    symbol_id: &str,
    sir_hash: &str,
    sir_json: &str,
    updated_at: i64,
) {
    let version = store
        .record_sir_version_if_changed(
            symbol_id,
            sir_hash,
            "mock",
            "mock-model",
            sir_json,
            updated_at,
            None,
        )
        .expect("record SIR history");
    store
        .write_sir_blob(symbol_id, sir_json)
        .expect("write SIR blob");
    store
        .upsert_sir_meta(SirMetaRecord {
            id: symbol_id.to_owned(),
            sir_hash: sir_hash.to_owned(),
            sir_version: version.version,
            provider: "mock".to_owned(),
            model: "mock-model".to_owned(),
            generation_pass: "scan".to_owned(),
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: version.updated_at,
            sir_status: "fresh".to_owned(),
            last_error: None,
            last_attempt_at: version.updated_at,
        })
        .expect("upsert SIR metadata");
}

fn set_symbol_access(
    store: &SqliteStore,
    symbol_id: &str,
    access_count: i64,
    last_accessed_at: Option<i64>,
) {
    store
        .conn
        .lock()
        .expect("lock store connection")
        .execute(
            "UPDATE symbols SET access_count = ?2, last_accessed_at = ?3 WHERE id = ?1",
            params![symbol_id, access_count, last_accessed_at],
        )
        .expect("update symbol access metadata");
}

fn project_note_record(
    note_id: &str,
    content: &str,
    tags: &[&str],
    updated_at: i64,
) -> ProjectNoteRecord {
    ProjectNoteRecord {
        note_id: note_id.to_owned(),
        content: content.to_owned(),
        content_hash: format!("hash-{note_id}"),
        source_type: "manual".to_owned(),
        source_agent: None,
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        entity_refs: Vec::new(),
        file_refs: Vec::new(),
        symbol_refs: Vec::new(),
        created_at: updated_at,
        updated_at,
        access_count: 0,
        last_accessed_at: None,
        is_archived: false,
    }
}

fn test_intent_record(
    file_path: &str,
    test_name: &str,
    intent_text: &str,
    symbol_id: Option<&str>,
) -> TestIntentRecord {
    TestIntentRecord {
        intent_id: String::new(),
        file_path: file_path.to_owned(),
        test_name: test_name.to_owned(),
        intent_text: intent_text.to_owned(),
        group_label: None,
        language: "rust".to_owned(),
        symbol_id: symbol_id.map(|value| value.to_owned()),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_100,
    }
}

fn write_intent_record(intent_id: &str, status: WriteIntentStatus) -> WriteIntent {
    WriteIntent {
        intent_id: intent_id.to_owned(),
        symbol_id: "sym-1".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        operation: IntentOperation::UpsertSir,
        status,
        payload_json: Some("{\"symbol_id\":\"sym-1\"}".to_owned()),
        created_at: 1_700_000_000,
        completed_at: None,
        error_message: None,
    }
}
fn write_intent_record_for_symbol(
    intent_id: &str,
    symbol_id: &str,
    status: WriteIntentStatus,
) -> WriteIntent {
    WriteIntent {
        intent_id: intent_id.to_owned(),
        symbol_id: symbol_id.to_owned(),
        file_path: "src/lib.rs".to_owned(),
        operation: IntentOperation::UpsertSir,
        status,
        payload_json: Some(format!("{{\"symbol_id\":\"{symbol_id}\"}}")),
        created_at: 1_700_000_000,
        completed_at: None,
        error_message: None,
    }
}

fn seed_live_symbol_cleanup_state(store: &SqliteStore, symbol_id: &str) {
    let mut record = symbol_record();
    record.id = symbol_id.to_owned();
    record.qualified_name = format!("demo::{symbol_id}");
    let mut neighbor = symbol_record_ts();
    neighbor.id = format!("{symbol_id}-neighbor");
    neighbor.qualified_name = format!("demo::{symbol_id}::neighbor");

    store
        .upsert_symbol(record.clone())
        .expect("upsert cleanup symbol");
    store
        .upsert_symbol(neighbor.clone())
        .expect("upsert cleanup neighbor");
    upsert_sir_state(
        store,
        symbol_id,
        format!("hash-{symbol_id}").as_str(),
        format!("{{\"intent\":\"cleanup {symbol_id}\"}}").as_str(),
        1_700_000_200,
    );
    store
        .upsert_symbol_embedding(embedding_record(
            symbol_id,
            format!("hash-{symbol_id}").as_str(),
            vec![1.0, 0.0],
        ))
        .expect("upsert cleanup embedding");
    store
        .upsert_edges(&[calls_edge(
            symbol_id,
            neighbor.qualified_name.as_str(),
            record.file_path.as_str(),
        )])
        .expect("upsert cleanup edge");
    store
        .populate_symbol_neighbors(record.file_path.as_str())
        .expect("populate cleanup neighbors");
    store
        .enqueue_sir_request(symbol_id)
        .expect("enqueue cleanup request");
    store
        .create_write_intent(&write_intent_record_for_symbol(
            format!("intent-{symbol_id}").as_str(),
            symbol_id,
            WriteIntentStatus::Pending,
        ))
        .expect("create cleanup intent");
    store
        .insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: symbol_id.to_owned(),
            timestamp: 1_700_000_300,
            prompt_hash: format!("prompt-{symbol_id}"),
            prompt_hash_previous: Some(format!("prompt-prev-{symbol_id}")),
            trigger: "cleanup-test".to_owned(),
            source_changed: true,
            neighbor_changed: false,
            config_changed: false,
            generation_model: Some("mock-model".to_owned()),
            generation_pass: Some("scan".to_owned()),
            delta_sem: Some(0.25),
        })
        .expect("insert cleanup fingerprint history");
    let contract_id = store
        .insert_intent_contract(
            symbol_id,
            "must",
            "Preserve cleanup semantics",
            None,
            "tests",
        )
        .expect("insert cleanup contract");
    store
        .insert_intent_violation(contract_id, symbol_id, 1, "semantic_drift", Some(0.8), None)
        .expect("insert cleanup violation");
    store
        .replace_test_intents_for_file(
            "tests/cleanup_test.rs",
            &[test_intent_record(
                "tests/cleanup_test.rs",
                "test_cleanup_symbol",
                "cleans symbol state",
                Some(symbol_id),
            )],
        )
        .expect("insert cleanup test intent");
    store
        .replace_community_snapshot(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            1_700_000_400,
            &[CommunitySnapshotRecord {
                snapshot_id: "ignored".to_owned(),
                symbol_id: symbol_id.to_owned(),
                community_id: 7,
                captured_at: 0,
            }],
        )
        .expect("insert cleanup community snapshot");
    store
        .upsert_drift_results(&[DriftResultRecord {
            result_id: format!("drift-{symbol_id}"),
            symbol_id: symbol_id.to_owned(),
            file_path: record.file_path.clone(),
            symbol_name: record.qualified_name.clone(),
            drift_type: "semantic".to_owned(),
            drift_magnitude: Some(0.4),
            current_sir_hash: Some(format!("hash-{symbol_id}")),
            baseline_sir_hash: Some(format!("hash-{symbol_id}-old")),
            commit_range_start: Some("aaaa".to_owned()),
            commit_range_end: Some("bbbb".to_owned()),
            drift_summary: Some("cleanup drift".to_owned()),
            detail_json: "{\"kind\":\"semantic\"}".to_owned(),
            detected_at: 1_700_000_500,
            is_acknowledged: false,
        }])
        .expect("insert cleanup drift result");
    store
        .insert_audit_finding(NewAuditFinding {
            symbol_id: symbol_id.to_owned(),
            audit_type: "symbol".to_owned(),
            severity: "high".to_owned(),
            category: "cleanup".to_owned(),
            certainty: "confirmed".to_owned(),
            trigger_condition: "cleanup trigger".to_owned(),
            impact: "cleanup impact".to_owned(),
            description: "cleanup description".to_owned(),
            related_symbols: Vec::new(),
            model: "tests".to_owned(),
            provider: "manual".to_owned(),
            reasoning: Some("cleanup reasoning".to_owned()),
            status: "open".to_owned(),
        })
        .expect("insert cleanup audit finding");

    let conn = store.conn.lock().expect("lock store connection");
    conn.execute(
        r#"
        INSERT INTO sir_quality (
            sir_id,
            specificity,
            behavioral_depth,
            error_coverage,
            length_score,
            composite_quality,
            confidence_percentile,
            normalized_quality,
            computed_at
        )
        VALUES (?1, 0.8, 0.7, 0.6, 0.9, 0.75, 0.5, 0.72, 1700000600)
        "#,
        params![symbol_id],
    )
    .expect("insert cleanup sir_quality");
    conn.execute(
        r#"
        INSERT INTO intent_snapshots (snapshot_id, git_commit, created_at, scope, symbol_count, deep_count)
        VALUES ('cleanup-snapshot', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1700000700, 'local', 1, 1)
        "#,
        [],
    )
    .expect("insert cleanup snapshot");
    conn.execute(
        r#"
        INSERT INTO intent_snapshot_entries (
            snapshot_id, symbol_id, qualified_name, file_path, signature_fingerprint, sir_json,
            generation_pass, was_deep_scanned
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'scan', 0)
        "#,
        params![
            "cleanup-snapshot",
            symbol_id,
            record.qualified_name,
            record.file_path,
            record.signature_fingerprint,
            format!("{{\"intent\":\"snapshot {symbol_id}\"}}"),
        ],
    )
    .expect("insert cleanup snapshot entry");
    conn.execute(
        r#"
        INSERT INTO metrics_cascade (
            epicenter_symbol_id, chain_json, total_hops, max_delta_sem, detected_at
        )
        VALUES (?1, '{"chain":[]}', 1, 0.2, 1700000800)
        "#,
        params![symbol_id],
    )
    .expect("insert cleanup cascade metric");
}

fn count_symbol_rows(store: &SqliteStore, sql: &str, symbol_id: &str) -> i64 {
    store
        .conn
        .lock()
        .expect("lock store connection")
        .query_row(sql, params![symbol_id], |row| row.get(0))
        .expect("count symbol rows")
}

fn assert_live_symbol_cleanup_empty(store: &SqliteStore, symbol_id: &str) {
    for sql in [
        "SELECT COUNT(*) FROM sir_embeddings WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM sir_history WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM write_intents WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM sir_requests WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM sir_fingerprint_history WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM symbol_edges WHERE source_id = ?1",
        "SELECT COUNT(*) FROM symbol_neighbors WHERE symbol_id = ?1 OR neighbor_id = ?1",
        "SELECT COUNT(*) FROM intent_violations WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM intent_contracts WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM test_intents WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM community_snapshot WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM drift_results WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM sir_quality WHERE sir_id = ?1",
        "SELECT COUNT(*) FROM sir_audit WHERE symbol_id = ?1",
        "SELECT COUNT(*) FROM sir WHERE id = ?1",
        "SELECT COUNT(*) FROM symbols WHERE id = ?1",
    ] {
        assert_eq!(count_symbol_rows(store, sql, symbol_id), 0, "{sql}");
    }
}

fn assert_preserved_symbol_history_retained(store: &SqliteStore, symbol_id: &str) {
    assert_eq!(
        count_symbol_rows(
            store,
            "SELECT COUNT(*) FROM intent_snapshot_entries WHERE symbol_id = ?1",
            symbol_id,
        ),
        1
    );
    assert_eq!(
        count_symbol_rows(
            store,
            "SELECT COUNT(*) FROM metrics_cascade WHERE epicenter_symbol_id = ?1",
            symbol_id,
        ),
        1
    );
}

fn calls_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
    SymbolEdge {
        source_id: source_id.to_owned(),
        target_qualified_name: target.to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: file_path.to_owned(),
    }
}

fn depends_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
    SymbolEdge {
        source_id: source_id.to_owned(),
        target_qualified_name: target.to_owned(),
        edge_kind: EdgeKind::DependsOn,
        file_path: file_path.to_owned(),
    }
}

fn type_ref_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
    SymbolEdge {
        source_id: source_id.to_owned(),
        target_qualified_name: target.to_owned(),
        edge_kind: EdgeKind::TypeRef,
        file_path: file_path.to_owned(),
    }
}

fn implements_edge(source_id: &str, target: &str, file_path: &str) -> SymbolEdge {
    SymbolEdge {
        source_id: source_id.to_owned(),
        target_qualified_name: target.to_owned(),
        edge_kind: EdgeKind::Implements,
        file_path: file_path.to_owned(),
    }
}

mod audit;
mod basic;
mod neighbors;
mod notes_and_analysis;
mod reconcile;
mod schema_and_intents;
mod sir;

#[test]
fn sqlite_store_implements_split_store_traits() {
    fn assert_store_traits<
        T: SymbolCatalogStore
            + SymbolRelationStore
            + SirStateStore
            + SirHistoryStore
            + AuditStore
            + SemanticIndexStore
            + ThresholdStore
            + ProjectNoteStore
            + ProjectNoteEmbeddingStore
            + CouplingStateStore
            + DriftStore
            + TestIntentStore
            + Store,
    >() {
    }

    assert_store_traits::<SqliteStore>();
}
