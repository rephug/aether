use super::*;

pub(super) fn declaration(id: &str, file: &str, language: &str, name: &str) -> SymbolRecord {
    SymbolRecord {
        id: id.to_owned(),
        file_path: file.to_owned(),
        language: language.to_owned(),
        kind: "function".to_owned(),
        qualified_name: name.to_owned(),
        signature_fingerprint: format!("signature-{id}"),
        last_seen_at: 1,
    }
}

pub(super) fn reference(
    source: &str,
    file: &str,
    name: &str,
    target_id: Option<&str>,
    target_file: Option<&str>,
) -> SymbolEdge {
    SymbolEdge {
        source_id: source.to_owned(),
        target_qualified_name: name.to_owned(),
        edge_kind: EdgeKind::Calls,
        file_path: file.to_owned(),
        target_symbol_id: target_id.map(str::to_owned),
        target_file_path: target_file.map(str::to_owned),
    }
}

pub(super) fn seed_current_catalog(
    workspace: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = SqliteStore::open(workspace)?;
    for symbol in [
        declaration("caller", "src/main.rs", "rust", "caller"),
        declaration("target", "src/dep.rs", "rust", "helper"),
        declaration("ts-caller", "src/app.ts", "typescript", "caller"),
        declaration("ts-target", "src/dep.ts", "typescript", "helper"),
    ] {
        store.upsert_symbol(symbol)?;
    }
    store.upsert_edges(&[
        reference("caller", "src/main.rs", "helper", None, None),
        reference("caller", "src/main.rs", "helper", Some("target"), None),
        reference("caller", "src/main.rs", "helper", None, Some("src/dep.rs")),
        reference("ts-caller", "src/app.ts", "helper", Some("ts-target"), None),
    ])?;
    store.populate_symbol_neighbors("src/main.rs")?;
    store.populate_symbol_neighbors("src/app.ts")?;
    store.write_sir_blob("caller", r#"{"intent":"preserve annotation"}"#)?;
    store.upsert_sir_meta(SirMetaRecord {
        id: "caller".to_owned(),
        ..sir_meta_record()
    })?;
    store.insert_intent_contract("caller", "must", "preserve contract", None, "human")?;
    drop(store);
    Ok(())
}

/// Preserve the actual v24 shape and a ready native-projection receipt.
fn legacy_catalog(
    workspace: &std::path::Path,
) -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
    seed_current_catalog(workspace)?;
    downgrade_catalog_to_v24(workspace)
}

pub(super) fn downgrade_catalog_to_v24(
    workspace: &std::path::Path,
) -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
    let connection = rusqlite::Connection::open(workspace.join(".aether/meta.sqlite"))?;
    connection.execute_batch(
        "DROP TRIGGER reject_legacy_rust_target_proof_insert;
         DROP TRIGGER reject_legacy_rust_target_proof_update;
         DROP TRIGGER reject_legacy_rust_source_insert;
         DROP TRIGGER reject_legacy_rust_source_update;
         ALTER TABLE symbol_edges DROP COLUMN rust_proof_version;
         ALTER TABLE symbol_edges DROP COLUMN rust_proof_generation;
         DELETE FROM schema_version WHERE component = 'rust_target_provenance';
         UPDATE schema_version SET version = 24 WHERE component = 'core';
         UPDATE schema_version SET version = 1 WHERE component = 'graph_target_provenance';
         PRAGMA user_version = 24;",
    )?;
    Ok(connection)
}

#[test]
fn schema_25_deproves_existing_rust_hints_without_losing_annotations()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let legacy = legacy_catalog(temp.path())?;
    let readonly = SqliteStore::open_readonly(temp.path())?;
    assert!(!readonly.has_graph_provenance_schema()?);
    assert!(matches!(
        readonly.list_graph_dependency_edges(),
        Err(StoreError::Compatibility(_))
    ));
    assert!(readonly.get_symbol_record("caller")?.is_some());
    let sir = readonly.read_sir_blob("caller")?;
    let metadata = readonly.get_sir_meta("caller")?;
    let contracts = readonly.list_intent_contracts_for_symbol("caller", false)?;
    assert_eq!(contracts.len(), 1);
    let before: u32 = legacy.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(before, 24);
    let proved_before: u32 = legacy.query_row(
        "SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller'
         AND (target_symbol_id IS NOT NULL OR target_file_path IS NOT NULL)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(proved_before, 2);
    assert!(!crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);
    drop(readonly);

    let upgraded = SqliteStore::open(temp.path())?;
    assert!(upgraded.has_graph_provenance_schema()?);
    assert_eq!(upgraded.read_sir_blob("caller")?, sir);
    assert_eq!(upgraded.get_sir_meta("caller")?, metadata);
    let contract = upgraded.list_intent_contracts_for_symbol("caller", false)?;
    assert_eq!(contract.len(), 1);
    assert_eq!(contract[0].id, contracts[0].id);
    assert_eq!(contract[0].clause_text, "preserve contract");
    assert_eq!(contract[0].symbol_id, "caller");
    let references =
        upgraded.list_symbol_edges_for_source_and_kinds("caller", &[EdgeKind::Calls])?;
    assert_eq!(
        references.len(),
        1,
        "proof variants collapse onto their unproved reference"
    );
    assert_eq!(references[0].target_qualified_name, "helper");
    assert!(references[0].target_symbol_id.is_none());
    assert!(references[0].target_file_path.is_none());
    let resolved = upgraded.list_graph_dependency_edges()?;
    assert!(
        resolved
            .iter()
            .all(|edge| edge.source_symbol_id != "caller")
    );
    assert!(
        resolved
            .iter()
            .any(|edge| edge.source_symbol_id == "ts-caller")
    );
    let neighbors: u32 = legacy.query_row(
        "SELECT COUNT(*) FROM symbol_neighbors WHERE symbol_id = 'caller' OR neighbor_id = 'caller'",
        [], |row| row.get(0),
    )?;
    assert_eq!(neighbors, 0);
    let native_receipt: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(native_receipt, 0);
    assert!(!crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);

    // A current structural writer can regenerate proof. Opening again must not
    // erase that newly extracted evidence or change preserved user state.
    upgraded.upsert_edges(&[reference(
        "caller",
        "src/main.rs",
        "helper",
        Some("target"),
        None,
    )])?;
    drop(upgraded);
    let reopened = SqliteStore::open(temp.path())?;
    assert!(
        reopened
            .list_graph_dependency_edges()?
            .iter()
            .any(|edge| edge.source_symbol_id == "caller")
    );
    assert_eq!(reopened.read_sir_blob("caller")?, sir);
    Ok(())
}

#[test]
fn schema_25_fences_an_already_open_legacy_rust_writer() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let legacy = legacy_catalog(temp.path())?;
    let mut old_insert = legacy.prepare(
        "INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path, target_symbol_id, target_file_path)
         VALUES (?1, ?2, 'calls', ?3, ?4, ?5)",
    )?;
    let current = SqliteStore::open(temp.path())?;
    for (target_id, target_file) in [(Some("target"), None), (None, Some("src/dep.rs"))] {
        let error = old_insert
            .execute(params![
                "caller",
                "helper",
                "src/main.rs",
                target_id,
                target_file
            ])
            .expect_err("old proof writer must be fenced");
        assert!(error.to_string().contains("restart older Aether"));
    }
    let error = legacy
        .execute(
            "UPDATE symbol_edges SET target_symbol_id = 'target' WHERE source_id = 'caller'",
            [],
        )
        .expect_err("legacy proof update must be fenced too");
    assert!(error.to_string().contains("restart older Aether"));
    let error = legacy
        .execute(
            "UPDATE symbol_edges SET target_file_path = 'src/dep.rs' WHERE source_id = 'caller'",
            [],
        )
        .expect_err("legacy file proof update must be fenced too");
    assert!(error.to_string().contains("restart older Aether"));
    let error = old_insert
        .execute(params![
            "orphan",
            "helper",
            "src/orphan.rs",
            Some("target"),
            None::<&str>
        ])
        .expect_err("Rust file hints are fenced before a source row exists");
    assert!(error.to_string().contains("restart older Aether"));
    old_insert.execute(params![
        "caller",
        "raw_unknown",
        "src/main.rs",
        None::<&str>,
        None::<&str>
    ])?;
    legacy.execute(
        "INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path, target_symbol_id)
         VALUES ('ts-caller', 'helper', 'type_ref', 'src/app.ts', 'ts-target')",
        [],
    )?;
    // A legacy UPDATE must not borrow the current policy tag from a raw row
    // inserted by the new writer. Its generation must advance on a current write.
    current.upsert_edges(&[reference(
        "caller",
        "src/main.rs",
        "current_raw",
        None,
        None,
    )])?;
    for column in ["target_symbol_id", "target_file_path"] {
        let statement = format!(
            "UPDATE symbol_edges SET {column} = 'untrusted' WHERE source_id = 'caller' AND target_qualified_name = 'current_raw'"
        );
        let error = legacy
            .execute(&statement, [])
            .expect_err("legacy mutation of a current raw row must be fenced");
        assert!(error.to_string().contains("fresh write generation"));
    }
    // Unknown-source proofs are not positively identified as non-Rust.
    let error = old_insert
        .execute(params![
            "unknown",
            "helper",
            "custom/source",
            Some("target"),
            None::<&str>
        ])
        .expect_err("unknown-source proof must be fenced");
    assert!(error.to_string().contains("current writer"));
    let error = legacy
        .execute(
            "UPDATE symbols SET language = 'rust' WHERE id = 'ts-caller'",
            [],
        )
        .expect_err("legacy non-Rust proof cannot become Rust proof through a language edit");
    assert!(error.to_string().contains("legacy target proof"));
    current.upsert_edges(&[reference(
        "caller",
        "src/main.rs",
        "helper",
        Some("target"),
        None,
    )])?;
    current.upsert_edges(&[reference(
        "caller",
        "src/main.rs",
        "helper",
        Some("target"),
        None,
    )])?;
    let generation: u32 = legacy.query_row("SELECT rust_proof_generation FROM symbol_edges WHERE source_id = 'caller' AND target_symbol_id = 'target'", [], |row| row.get(0))?;
    assert_eq!(
        generation, 2,
        "current upserts advance the proof generation"
    );
    let tag: u32 = legacy.query_row(
        "SELECT rust_proof_version FROM symbol_edges WHERE source_id = 'caller' AND target_symbol_id = 'target'",
        [], |row| row.get(0),
    )?;
    assert_eq!(tag, crate::graph_resolution::RUST_TARGET_PROOF_VERSION);
    Ok(())
}

#[test]
fn graph_reads_require_the_current_rust_proof_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = SqliteStore::open(temp.path())?;
    let connection = rusqlite::Connection::open(temp.path().join(".aether/meta.sqlite"))?;
    connection.execute(
        "UPDATE schema_version SET version = 1 WHERE component = 'graph_target_provenance'",
        [],
    )?;
    connection.execute(
        "DELETE FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
    )?;
    assert!(!store.has_graph_provenance_schema()?);
    assert!(matches!(
        store.list_graph_dependency_edges(),
        Err(StoreError::Compatibility(_))
    ));
    assert!(!crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);
    assert!(matches!(
        crate::graph_resolution::require_graph_projection_provenance(temp.path()),
        Err(StoreError::Compatibility(_))
    ));
    connection.execute(
        "INSERT INTO schema_version VALUES ('rust_target_provenance', 3, unixepoch())",
        [],
    )?;
    assert!(matches!(
        store.has_graph_provenance_schema(),
        Err(StoreError::Compatibility(_))
    ));
    assert!(matches!(
        crate::graph_resolution::graph_projection_provenance_ready(temp.path()),
        Err(StoreError::Compatibility(_))
    ));
    let future: u32 = connection.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(future, 3);
    Ok(())
}

#[test]
fn schema_25_downgrades_orphan_proof_before_a_rust_source_can_claim_it()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let legacy = legacy_catalog(temp.path())?;
    legacy.execute(
        "INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path, target_symbol_id)
         VALUES ('unknown-source', 'helper', 'calls', 'custom/source', 'target')", [],
    )?;
    let store = SqliteStore::open(temp.path())?;
    store.upsert_symbol(declaration(
        "unknown-source",
        "custom/source",
        "rust",
        "later",
    ))?;
    let references =
        store.list_symbol_edges_for_source_and_kinds("unknown-source", &[EdgeKind::Calls])?;
    assert_eq!(references.len(), 1);
    assert!(references[0].target_symbol_id.is_none());
    assert!(references[0].target_file_path.is_none());
    assert!(
        !store
            .list_graph_dependency_edges()?
            .iter()
            .any(|edge| edge.source_symbol_id == "unknown-source")
    );
    Ok(())
}

#[test]
fn schema_25_rejects_future_proof_policy_before_downgrading_existing_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let legacy = legacy_catalog(temp.path())?;
    legacy.execute(
        "INSERT INTO schema_version VALUES ('rust_target_provenance', 3, unixepoch())",
        [],
    )?;
    let result = SqliteStore::open(temp.path());
    assert!(
        matches!(result, Err(StoreError::Compatibility(message)) if message.contains("unsupported Rust target-proof version"))
    );
    let version: u32 = legacy.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 24);
    let receipt: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(receipt, 3);
    let proof_rows: u32 = legacy.query_row("SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller' AND (target_symbol_id IS NOT NULL OR target_file_path IS NOT NULL)", [], |row| row.get(0))?;
    assert_eq!(
        proof_rows, 2,
        "the failed migration must not clear evidence"
    );
    let added_columns: u32 = legacy.query_row("SELECT COUNT(*) FROM pragma_table_info('symbol_edges') WHERE name IN ('rust_proof_version', 'rust_proof_generation')", [], |row| row.get(0))?;
    assert_eq!(added_columns, 0);
    legacy.execute(
        "UPDATE schema_version SET version = -1 WHERE component = 'rust_target_provenance'",
        [],
    )?;
    let result = SqliteStore::open(temp.path());
    assert!(
        matches!(result, Err(StoreError::Compatibility(message)) if message.contains("unsupported Rust target-proof version"))
    );
    let receipt: i64 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(receipt, -1);
    let readonly = SqliteStore::open_readonly(temp.path())?;
    assert_eq!(
        readonly.read_sir_blob("caller")?.as_deref(),
        Some(r#"{"intent":"preserve annotation"}"#)
    );
    Ok(())
}

#[tokio::test]
async fn schema_25_replays_a_cached_native_graph_without_legacy_rust_proofs()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    seed_current_catalog(temp.path())?;
    // Hold this handle across the SQL upgrade: the subsequent writable opener
    // must reuse it and replay the pending projection instead of accepting the
    // cached native store as already current.
    let original = SurrealGraphStore::open(temp.path()).await?;
    assert!(
        original
            .get_dependencies("caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "target")
    );
    assert!(
        original
            .get_dependencies("ts-caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "ts-target")
    );
    assert!(crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);

    let legacy = downgrade_catalog_to_v24(temp.path())?;
    assert!(!crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);
    assert!(matches!(
        SurrealGraphStore::open_readonly(temp.path()).await,
        Err(StoreError::Compatibility(_))
    ));
    let upgraded = SqliteStore::open(temp.path())?;
    let pending: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(pending, 0);
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .all(|edge| edge.source_symbol_id != "caller")
    );
    // A pre-upgrade native handle is not retroactively fenced by a SQL trigger.
    // New readonly openings must reject it until writable replay completes.
    assert!(
        original
            .get_dependencies("caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "target")
    );
    assert!(matches!(
        SurrealGraphStore::open_readonly(temp.path()).await,
        Err(StoreError::Compatibility(_))
    ));

    let cached = SurrealGraphStore::open(temp.path()).await?;
    assert!(cached.get_dependencies("caller").await?.is_empty());
    assert!(original.get_dependencies("caller").await?.is_empty());
    assert!(
        cached
            .get_dependencies("ts-caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "ts-target")
    );
    assert!(crate::graph_resolution::graph_projection_provenance_ready(
        temp.path()
    )?);
    let readonly = SurrealGraphStore::open_readonly(temp.path()).await?;
    assert!(readonly.get_dependencies("caller").await?.is_empty());
    assert!(
        readonly
            .get_dependencies("ts-caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "ts-target")
    );
    Ok(())
}

#[test]
fn schema_25_preserves_references_with_empty_target_hints() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    seed_current_catalog(temp.path())?;
    let current = SqliteStore::open(temp.path())?;
    let cases = [
        ("empty-id", Some(""), None),
        ("empty-file", None, Some("")),
        ("empty-both", Some(""), Some("")),
    ];
    for (name, id, file) in cases {
        // The public store/serde model accepts Some("") even though the parser
        // does not currently emit it. These are retained historical references.
        current.upsert_edges(&[reference("caller", "src/main.rs", name, id, file)])?;
    }
    let before = current.list_symbol_edges_for_source_and_kinds("caller", &[EdgeKind::Calls])?;
    for (name, id, file) in cases {
        let row = before
            .iter()
            .find(|row| row.target_qualified_name == name)
            .expect("accepted reference");
        assert_eq!(row.target_symbol_id.as_deref(), id);
        assert_eq!(row.target_file_path.as_deref(), file);
    }
    drop(current);
    let _legacy = downgrade_catalog_to_v24(temp.path())?;
    let upgraded = SqliteStore::open(temp.path())?;
    let after = upgraded.list_symbol_edges_for_source_and_kinds("caller", &[EdgeKind::Calls])?;
    for (name, _, _) in cases {
        let retained = after
            .iter()
            .find(|row| row.target_qualified_name == name)
            .expect("migration must retain the reference despite empty proof hints");
        assert_eq!(retained.source_id, "caller");
        assert_eq!(retained.file_path, "src/main.rs");
        assert_eq!(retained.edge_kind, EdgeKind::Calls);
        assert!(retained.target_symbol_id.is_none());
        assert!(retained.target_file_path.is_none());
    }
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .all(|edge| edge.source_symbol_id != "caller")
    );
    Ok(())
}
