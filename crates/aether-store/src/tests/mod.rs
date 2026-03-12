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

mod basic;
mod notes_and_analysis;
mod reconcile;
mod schema_and_intents;
mod sir;
