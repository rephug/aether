use super::rust_proof_upgrade::{
    declaration, downgrade_catalog_to_v24, reference, seed_current_catalog,
};
use super::*;

/// Reconstruct the actual policy-1 row tags and all four writer fences, not just
/// its receipt. The physical eight-column schema remains core version 25.
fn downgrade_catalog_to_policy1(
    workspace: &std::path::Path,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let connection = Connection::open(workspace.join(".aether/meta.sqlite"))?;
    connection.execute_batch(
        "DROP TRIGGER reject_legacy_rust_target_proof_insert;
         DROP TRIGGER reject_legacy_rust_target_proof_update;
         DROP TRIGGER reject_legacy_rust_source_insert;
         DROP TRIGGER reject_legacy_rust_source_update;
         UPDATE symbol_edges SET rust_proof_version = 1;
         UPDATE schema_version SET version = 1 WHERE component = 'rust_target_provenance';
         UPDATE schema_version SET version = 1 WHERE component = 'graph_target_provenance';
         CREATE TRIGGER reject_legacy_rust_target_proof_insert
         BEFORE INSERT ON symbol_edges
         WHEN (NEW.target_symbol_id IS NOT NULL OR NEW.target_file_path IS NOT NULL)
          AND (NEW.rust_proof_version <> 1 OR NEW.rust_proof_generation <= 0)
          AND (LOWER(TRIM(NEW.file_path)) LIKE '%.rs' OR NOT EXISTS (
              SELECT 1 FROM symbols WHERE id = NEW.source_id AND LOWER(TRIM(language)) NOT IN ('rust', '')
          ))
         BEGIN
             SELECT RAISE(ABORT, 'Rust target proof requires a current writer; restart older Aether processes and reindex');
         END;
         CREATE TRIGGER reject_legacy_rust_target_proof_update
         BEFORE UPDATE ON symbol_edges
         WHEN (NEW.target_symbol_id IS NOT NULL OR NEW.target_file_path IS NOT NULL)
          AND (NEW.rust_proof_version <> 1 OR NEW.rust_proof_generation <= OLD.rust_proof_generation)
          AND (LOWER(TRIM(NEW.file_path)) LIKE '%.rs' OR NOT EXISTS (
              SELECT 1 FROM symbols WHERE id = NEW.source_id AND LOWER(TRIM(language)) NOT IN ('rust', '')
          ))
         BEGIN
             SELECT RAISE(ABORT, 'Rust target proof requires a fresh write generation; restart older Aether processes and reindex');
         END;
         CREATE TRIGGER reject_legacy_rust_source_insert
         BEFORE INSERT ON symbols
         WHEN LOWER(TRIM(NEW.language)) = 'rust' AND EXISTS (
             SELECT 1 FROM symbol_edges e WHERE e.source_id = NEW.id
               AND (e.target_symbol_id IS NOT NULL OR e.target_file_path IS NOT NULL)
               AND (e.rust_proof_version <> 1 OR e.rust_proof_generation <= 0)
         )
         BEGIN
             SELECT RAISE(ABORT, 'Rust source has legacy target proof; reindex its edges before changing language');
         END;
         CREATE TRIGGER reject_legacy_rust_source_update
         BEFORE UPDATE OF id, language ON symbols
         WHEN LOWER(TRIM(NEW.language)) = 'rust' AND EXISTS (
             SELECT 1 FROM symbol_edges e WHERE e.source_id = NEW.id
               AND (e.target_symbol_id IS NOT NULL OR e.target_file_path IS NOT NULL)
               AND (e.rust_proof_version <> 1 OR e.rust_proof_generation <= 0)
         )
         BEGIN
             SELECT RAISE(ABORT, 'Rust source has legacy target proof; reindex its edges before changing language');
         END;",
    )?;
    Ok(connection)
}

fn policy1_catalog(workspace: &std::path::Path) -> Result<Connection, Box<dyn std::error::Error>> {
    seed_current_catalog(workspace)?;
    downgrade_catalog_to_policy1(workspace)
}

fn edge_columns(connection: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement =
        connection.prepare("SELECT name FROM pragma_table_info('symbol_edges') ORDER BY cid")?;
    Ok(statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

#[test]
fn rust_policy2_upgrades_policy1_without_changing_core25_or_user_state()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    seed_current_catalog(temp.path())?;
    let current = SqliteStore::open(temp.path())?;
    for (name, id, file) in [
        ("empty-id", Some(""), None),
        ("empty-file", None, Some("")),
        ("empty-both", Some(""), Some("")),
    ] {
        current.upsert_edges(&[reference("caller", "src/main.rs", name, id, file)])?;
    }
    current.upsert_edges(&[reference(
        "pending",
        "custom/source",
        "helper",
        Some("target"),
        None,
    )])?;
    drop(current);
    let legacy = downgrade_catalog_to_policy1(temp.path())?;
    let columns = edge_columns(&legacy)?;
    let mut old_insert = legacy.prepare(
        "INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path, target_symbol_id, target_file_path, rust_proof_version, rust_proof_generation)
         VALUES ('caller', 'old-writer-control', 'calls', 'src/main.rs', 'target', NULL, 1, 1)
         ON CONFLICT DO UPDATE SET file_path = excluded.file_path, rust_proof_version = 1, rust_proof_generation = symbol_edges.rust_proof_generation + 1",
    )?;
    assert_eq!(
        old_insert.execute([])?,
        1,
        "the real policy-1 protocol is accepted before upgrade"
    );
    let mut old_id_update = legacy.prepare(
        "UPDATE symbol_edges SET target_symbol_id = 'target', rust_proof_version = 1, rust_proof_generation = rust_proof_generation + 1
         WHERE source_id = 'caller' AND target_qualified_name = 'helper' AND target_symbol_id IS NULL AND target_file_path IS NULL",
    )?;
    let mut old_file_update = legacy.prepare(
        "UPDATE symbol_edges SET target_file_path = 'src/dep.rs', rust_proof_version = 1, rust_proof_generation = rust_proof_generation + 1
         WHERE source_id = 'caller' AND target_qualified_name = 'helper' AND target_symbol_id IS NULL AND target_file_path IS NULL",
    )?;
    let readonly = SqliteStore::open_readonly(temp.path())?;
    assert!(!readonly.has_graph_provenance_schema()?);
    assert!(!readonly.has_graph_projection_provenance()?);
    assert!(matches!(
        readonly.list_graph_dependency_edges(),
        Err(StoreError::Compatibility(_))
    ));
    let symbol = readonly.get_symbol_record("caller")?;
    let sir = readonly.read_sir_blob("caller")?;
    let meta = readonly.get_sir_meta("caller")?;
    let contracts = readonly.list_intent_contracts_for_symbol("caller", false)?;
    let before_policy: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        before_policy, 1,
        "readonly access must not upgrade the catalog"
    );
    drop(readonly);

    let upgraded = SqliteStore::open(temp.path())?;
    assert_eq!(upgraded.get_schema_version()?.version, 25);
    assert_eq!(edge_columns(&legacy)?, columns);
    assert!(upgraded.has_graph_provenance_schema()?);
    assert!(!upgraded.has_graph_projection_provenance()?);
    assert_eq!(upgraded.get_symbol_record("caller")?, symbol);
    assert_eq!(upgraded.read_sir_blob("caller")?, sir);
    assert_eq!(upgraded.get_sir_meta("caller")?, meta);
    assert_eq!(
        upgraded.list_intent_contracts_for_symbol("caller", false)?,
        contracts
    );
    let policy: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(policy, 2);
    let references =
        upgraded.list_symbol_edges_for_source_and_kinds("caller", &[EdgeKind::Calls])?;
    for name in [
        "helper",
        "old-writer-control",
        "empty-id",
        "empty-file",
        "empty-both",
    ] {
        let retained = references
            .iter()
            .find(|edge| edge.target_qualified_name == name)
            .expect("historical reference retained");
        assert!(retained.target_symbol_id.is_none());
        assert!(retained.target_file_path.is_none());
    }
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .all(|edge| edge.source_symbol_id != "caller")
    );
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .any(|edge| edge.source_symbol_id == "ts-caller")
    );
    let neighbors: u32 = legacy.query_row("SELECT COUNT(*) FROM symbol_neighbors WHERE symbol_id = 'caller' OR neighbor_id = 'caller'", [], |row| row.get(0))?;
    assert_eq!(neighbors, 0);
    upgraded.upsert_symbol(declaration("pending", "custom/source", "rust", "later"))?;
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .all(|edge| edge.source_symbol_id != "pending")
    );
    for result in [
        old_insert.execute([]),
        old_id_update.execute([]),
        old_file_update.execute([]),
    ] {
        let error =
            result.expect_err("prepared policy-1 writer must fail even with advancing generations");
        assert!(error.to_string().contains("restart older Aether"));
    }
    upgraded.upsert_edges(&[reference(
        "caller",
        "src/main.rs",
        "helper",
        Some("target"),
        None,
    )])?;
    let writer_policy: u32 = legacy.query_row("SELECT rust_proof_version FROM symbol_edges WHERE source_id = 'caller' AND target_symbol_id = 'target'", [], |row| row.get(0))?;
    assert_eq!(writer_policy, 2);
    legacy.execute(
        "UPDATE schema_version SET migrated_at = 17 WHERE component = 'graph_target_provenance'",
        [],
    )?;
    drop(upgraded);
    let reopened = SqliteStore::open(temp.path())?;
    assert!(
        reopened
            .list_graph_dependency_edges()?
            .iter()
            .any(|edge| edge.source_symbol_id == "caller")
    );
    let unchanged: i64 = legacy.query_row(
        "SELECT migrated_at FROM schema_version WHERE component = 'graph_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        unchanged, 17,
        "current policy2 must not reset native replay on every open"
    );
    Ok(())
}

#[test]
fn rust_policy2_upgrades_missing_and_zero_receipts_at_core25()
-> Result<(), Box<dyn std::error::Error>> {
    for receipt in [None, Some(0)] {
        let temp = tempdir()?;
        let legacy = policy1_catalog(temp.path())?;
        if let Some(value) = receipt {
            legacy.execute(
                "UPDATE schema_version SET version = ?1 WHERE component = 'rust_target_provenance'",
                params![value],
            )?;
        } else {
            legacy.execute(
                "DELETE FROM schema_version WHERE component = 'rust_target_provenance'",
                [],
            )?;
        }
        let store = SqliteStore::open(temp.path())?;
        assert_eq!(store.get_schema_version()?.version, 25);
        assert!(store.has_graph_provenance_schema()?);
        assert!(!store.has_graph_projection_provenance()?);
        assert!(
            store
                .list_graph_dependency_edges()?
                .iter()
                .all(|edge| edge.source_symbol_id != "caller")
        );
        let current: u32 = legacy.query_row(
            "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(current, 2);
    }
    Ok(())
}

#[test]
fn rust_policy2_preserves_unknown_policy_at_core25_without_replacing_fences()
-> Result<(), Box<dyn std::error::Error>> {
    for unsupported in [3_i64, -1] {
        let temp = tempdir()?;
        let legacy = policy1_catalog(temp.path())?;
        legacy.execute(
            "UPDATE schema_version SET version = ?1 WHERE component = 'rust_target_provenance'",
            params![unsupported],
        )?;
        legacy.execute("DROP INDEX idx_symbols_qualified_name", [])?;
        let fence: String = legacy.query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'reject_legacy_rust_target_proof_insert'",
            [],
            |row| row.get(0),
        )?;
        let result = SqliteStore::open(temp.path());
        assert!(
            matches!(result, Err(StoreError::Compatibility(message)) if message.contains("unsupported Rust target-proof version"))
        );
        let current: i64 = legacy.query_row(
            "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(current, unsupported);
        let version: u32 = legacy.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(version, 25);
        let proof_rows: u32 = legacy.query_row("SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller' AND (target_symbol_id IS NOT NULL OR target_file_path IS NOT NULL)", [], |row| row.get(0))?;
        assert_eq!(proof_rows, 2);
        let same_fence: String = legacy.query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'reject_legacy_rust_target_proof_insert'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(same_fence, fence);
        let index_count: u32 = legacy.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_symbols_qualified_name'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            index_count, 0,
            "future policy rejection must not recreate additive indexes"
        );
        let readonly = SqliteStore::open_readonly(temp.path())?;
        assert!(readonly.read_sir_blob("caller")?.is_some());
        assert!(matches!(
            readonly.has_graph_projection_provenance(),
            Err(StoreError::Compatibility(_))
        ));
    }
    Ok(())
}

#[test]
fn physical_upgrade_does_not_trust_a_current_receipt_on_an_old_table_shape()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    seed_current_catalog(temp.path())?;
    let legacy = downgrade_catalog_to_v24(temp.path())?;
    legacy.execute(
        "INSERT INTO schema_version VALUES ('rust_target_provenance', 2, unixepoch())",
        [],
    )?;
    let upgraded = SqliteStore::open(temp.path())?;
    assert!(
        upgraded
            .list_graph_dependency_edges()?
            .iter()
            .all(|edge| edge.source_symbol_id != "caller")
    );
    let result = legacy.execute("INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path, target_symbol_id) VALUES ('caller', 'helper', 'calls', 'src/main.rs', 'target')", []);
    assert!(
        result.is_err(),
        "physical upgrade must install current writer fences too"
    );
    let policy: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'rust_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(policy, 2);
    Ok(())
}

#[tokio::test]
async fn rust_policy2_replays_cached_native_policy1_and_preserves_typescript()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    seed_current_catalog(temp.path())?;
    let original = SurrealGraphStore::open(temp.path()).await?;
    assert!(
        original
            .get_dependencies("caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "target")
    );
    let legacy = downgrade_catalog_to_policy1(temp.path())?;
    let readonly = SqliteStore::open_readonly(temp.path())?;
    assert!(!readonly.has_graph_provenance_schema()?);
    assert!(!readonly.has_graph_projection_provenance()?);
    assert!(matches!(
        SurrealGraphStore::open_readonly(temp.path()).await,
        Err(StoreError::Compatibility(_))
    ));
    drop(readonly);
    let store = SqliteStore::open(temp.path())?;
    assert!(store.has_graph_provenance_schema()?);
    assert!(!store.has_graph_projection_provenance()?);
    let native: u32 = legacy.query_row(
        "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(native, 0);
    assert!(matches!(
        SurrealGraphStore::open_readonly(temp.path()).await,
        Err(StoreError::Compatibility(_))
    ));
    // A held old native handle is not retroactively fenced by SQL receipts.
    assert!(
        original
            .get_dependencies("caller")
            .await?
            .iter()
            .any(|symbol| symbol.id == "target")
    );
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
    assert!(store.has_graph_provenance_schema()?);
    assert!(store.has_graph_projection_provenance()?);
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
fn current_delete_refuses_unavailable_policy_without_removing_references()
-> Result<(), Box<dyn std::error::Error>> {
    for policy in [None, Some(3)] {
        let temp = tempdir()?;
        seed_current_catalog(temp.path())?;
        let store = SqliteStore::open(temp.path())?;
        let connection = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
        if let Some(version) = policy {
            connection.execute(
                "UPDATE schema_version SET version = ?1 WHERE component = 'rust_target_provenance'",
                params![version],
            )?;
        } else {
            connection.execute(
                "DELETE FROM schema_version WHERE component = 'rust_target_provenance'",
                [],
            )?;
        }
        let before: u32 = connection.query_row(
            "SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller'",
            [],
            |row| row.get(0),
        )?;
        let raw_before: u32 = connection.query_row("SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller' AND target_symbol_id IS NULL AND target_file_path IS NULL", [], |row| row.get(0))?;
        assert!(matches!(
            store.delete_edges_for_file("src/main.rs"),
            Err(StoreError::Compatibility(_))
        ));
        let after: u32 = connection.query_row(
            "SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller'",
            [],
            |row| row.get(0),
        )?;
        let raw_after: u32 = connection.query_row("SELECT COUNT(*) FROM symbol_edges WHERE source_id = 'caller' AND target_symbol_id IS NULL AND target_file_path IS NULL", [], |row| row.get(0))?;
        assert_eq!(after, before);
        assert_eq!(raw_after, raw_before);
        assert!(raw_after > 0);
        assert!(store.read_sir_blob("caller")?.is_some());
        assert!(store.get_symbol_record("caller")?.is_some());
    }
    Ok(())
}
