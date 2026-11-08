use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use aether_store::{
    AuditStore, NewAuditFinding, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
};
use rusqlite::{Connection, OpenFlags};
use tempfile::tempdir;

fn audit_report(workspace: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aetherd"));
    command
        .arg("--workspace")
        .arg(workspace)
        .arg("audit-report")
        .env_clear();
    for name in ["PATH", "LD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.output().expect("run audit-report")
}

fn prepare_catalog(workspace: &Path, with_finding: bool) {
    let store = SqliteStore::open(workspace).expect("create fixture catalog");
    if with_finding {
        store
            .upsert_symbol(SymbolRecord {
                id: "audit-fixture".to_owned(),
                file_path: "src/worker.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "worker::process".to_owned(),
                signature_fingerprint: "fixture-signature".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("seed symbol");
        store
            .write_sir_blob("audit-fixture", r#"{"intent":"Process fixture input"}"#)
            .expect("seed explanation");
        store
            .insert_audit_finding(NewAuditFinding {
                symbol_id: "audit-fixture".to_owned(),
                audit_type: "symbol".to_owned(),
                severity: "high".to_owned(),
                category: "state".to_owned(),
                certainty: "confirmed".to_owned(),
                trigger_condition: "fixture trigger".to_owned(),
                impact: "fixture impact".to_owned(),
                description: "fixture observation".to_owned(),
                related_symbols: Vec::new(),
                model: "fixture".to_owned(),
                provider: "fixture".to_owned(),
                reasoning: None,
                status: "open".to_owned(),
            })
            .expect("seed audit finding");
    }
    drop(store);
    // An offline catalog can use DELETE journaling. Reading a report must not
    // switch its journal mode or create WAL sidecars.
    let conn = Connection::open(workspace.join(".aether/meta.sqlite")).expect("open fixture");
    conn.execute_batch("PRAGMA journal_mode = DELETE;")
        .expect("prepare offline catalog");
}

#[test]
fn audit_report_refuses_an_unindexed_workspace_without_creating_files() {
    let workspace = tempdir().expect("workspace");
    let output = audit_report(workspace.path());

    assert!(
        !output.status.success(),
        "missing catalog must not look clean"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("audit catalog"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(!workspace.path().join(".aether").exists());
}

#[test]
fn audit_report_reads_delete_mode_catalog_without_initializing_config_or_wal() {
    let workspace = tempdir().expect("workspace");
    prepare_catalog(workspace.path(), true);
    let database = workspace.path().join(".aether/meta.sqlite");
    let before = fs::read(&database).expect("snapshot catalog");

    let output = audit_report(workspace.path());
    assert!(output.status.success(), "{output:?}");
    let report = String::from_utf8(output.stdout).expect("UTF-8 report");
    for expected in [
        "Showing 1 of 1 matching findings.",
        "Finding ID: 1",
        "Symbol ID: audit-fixture",
        "File: src/worker.rs",
        "Trigger: fixture trigger",
        "Impact: fixture impact",
    ] {
        assert!(report.contains(expected), "missing {expected}: {report}");
    }
    assert!(!workspace.path().join(".aether/config.toml").exists());
    assert!(!workspace.path().join(".aether/meta.sqlite-wal").exists());
    assert!(!workspace.path().join(".aether/meta.sqlite-shm").exists());
    assert!(
        fs::read(database).expect("read catalog after report") == before,
        "audit-report changed the catalog bytes"
    );
}

#[test]
fn audit_report_does_not_upgrade_a_supported_older_catalog() {
    let workspace = tempdir().expect("workspace");
    prepare_catalog(workspace.path(), true);
    let database = workspace.path().join(".aether/meta.sqlite");
    // Reconstruct v22 by removing the table introduced by v23. The report's
    // required audit and symbol tables are already present in this version.
    let conn = Connection::open(&database).expect("open fixture");
    conn.execute_batch(
        "DROP TABLE retirement_cleanup;
         PRAGMA user_version = 22;
         UPDATE schema_version SET version = 22 WHERE component = 'core';",
    )
    .expect("prepare older catalog");
    drop(conn);
    let before = fs::read(&database).expect("snapshot older catalog");

    let output = audit_report(workspace.path());
    assert!(output.status.success(), "{output:?}");
    assert!(
        fs::read(&database).expect("read older catalog") == before,
        "audit-report changed the older catalog bytes"
    );
    let conn = Connection::open(&database).expect("inspect schema marker");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read schema marker");
    assert_eq!(version, 22);
    let cleanup_table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'retirement_cleanup'",
            [],
            |row| row.get(0),
        )
        .expect("inspect older schema");
    assert_eq!(cleanup_table_count, 0);
}

#[test]
fn audit_report_can_still_report_an_existing_empty_catalog() {
    let workspace = tempdir().expect("workspace");
    prepare_catalog(workspace.path(), false);

    let output = audit_report(workspace.path());
    assert!(output.status.success(), "{output:?}");
    let report = String::from_utf8(output.stdout).expect("UTF-8 report");
    assert!(report.contains("Showing 0 of 0 matching findings."));
    assert!(report.contains("No audit findings matched the requested filters."));
}

fn sidecar_state(path: &Path) -> Option<(usize, String)> {
    match fs::read(path) {
        Ok(bytes) => Some((bytes.len(), blake3::hash(&bytes).to_hex().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("read sidecar {}: {error}", path.display()),
    }
}

fn catalog_entries(workspace: &Path) -> BTreeSet<OsString> {
    fs::read_dir(workspace.join(".aether"))
        .expect("read fixture catalog directory")
        .map(|entry| entry.expect("read catalog entry").file_name())
        .collect()
}

#[test]
fn audit_report_sees_committed_wal_rows_while_a_writer_remains_open() {
    let workspace = tempdir().expect("workspace");
    prepare_catalog(workspace.path(), true);
    let database = workspace.path().join(".aether/meta.sqlite");
    let wal = workspace.path().join(".aether/meta.sqlite-wal");
    let shm = workspace.path().join(".aether/meta.sqlite-shm");
    let writer = Connection::open(&database).expect("open fixture writer");
    writer
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .expect("enable WAL without automatic checkpointing");
    let before_insert = fs::read(&database).expect("snapshot checkpointed database");
    writer
        .execute_batch(
            "INSERT INTO sir_audit (
                symbol_id, audit_type, severity, category, certainty, trigger_condition,
                impact, description, related_symbols, model, provider, status, created_at
             ) VALUES (
                'audit-fixture', 'symbol', 'high', 'state', 'confirmed', 'WAL trigger',
                'WAL impact', 'WAL-only committed finding', '[]', 'fixture', 'fixture',
                'open', 1700000001
             );",
        )
        .expect("commit a new finding into the WAL");
    let database_before = fs::read(&database).expect("snapshot database before reporting");
    assert!(
        database_before == before_insert,
        "the new committed finding must still be in the WAL, not the main database"
    );
    let wal_before = fs::read(&wal).expect("committed WAL exists");
    assert!(wal_before.len() > 32, "WAL must contain committed frames");
    let shm_before = sidecar_state(&shm);
    assert!(
        shm_before.is_some(),
        "active writer has a shared-memory index"
    );

    let output = audit_report(workspace.path());
    assert!(output.status.success(), "{output:?}");
    let report = String::from_utf8(output.stdout).expect("UTF-8 report");
    assert!(
        report.contains("Showing 2 of 2 matching findings."),
        "{report}"
    );
    assert!(report.contains("WAL-only committed finding"), "{report}");
    assert!(report.contains("Finding ID: 2"), "{report}");
    assert!(fs::read(&database).expect("read main database after report") == database_before);
    assert!(fs::read(&wal).expect("read WAL after report") == wal_before);
    assert!(!workspace.path().join(".aether/config.toml").exists());
    // SQLite readers can update the shared-memory WAL index/read marks. Capture
    // that bookkeeping separately; do not promise unchanged SHM bytes.
    eprintln!(
        "active WAL: shm before={shm_before:?}, after={:?}; main database and WAL bytes unchanged",
        sidecar_state(&shm)
    );
    let integrity: String = writer
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("check database and WAL integrity");
    let count: i64 = writer
        .query_row("SELECT COUNT(*) FROM sir_audit", [], |row| row.get(0))
        .expect("check committed findings");
    assert_eq!(integrity, "ok");
    assert_eq!(count, 2);
    // Keep this connection alive through every assertion so closing the final
    // writer cannot checkpoint/delete the WAL during the report measurements.
    drop(writer);
}

#[test]
fn audit_report_preserves_a_closed_wal_catalog_while_allowing_sqlite_sidecars() {
    let workspace = tempdir().expect("workspace");
    prepare_catalog(workspace.path(), true);
    let database = workspace.path().join(".aether/meta.sqlite");
    let wal = workspace.path().join(".aether/meta.sqlite-wal");
    let shm = workspace.path().join(".aether/meta.sqlite-shm");
    let writer = Connection::open(&database).expect("open fixture writer");
    writer
        .execute_batch("PRAGMA journal_mode = WAL;")
        .expect("enable persistent WAL mode");
    drop(writer);
    assert!(!wal.exists() && !shm.exists(), "cleanly closed WAL fixture");
    let before = fs::read(&database).expect("snapshot closed WAL database");
    let entries_before = catalog_entries(workspace.path());

    let output = audit_report(workspace.path());
    assert!(output.status.success(), "{output:?}");
    let report = String::from_utf8(output.stdout).expect("UTF-8 report");
    assert!(
        report.contains("Showing 1 of 1 matching findings."),
        "{report}"
    );
    assert!(fs::read(&database).expect("read database after report") == before);
    assert!(!workspace.path().join(".aether/config.toml").exists());
    // A read-only SQLite connection may create WAL/SHM files in a writable
    // directory. They are permitted here; unrelated report artifacts are not.
    let entries_after = catalog_entries(workspace.path());
    for entry in entries_after.difference(&entries_before) {
        assert!(matches!(
            entry.to_str(),
            Some("meta.sqlite-wal" | "meta.sqlite-shm")
        ));
    }
    eprintln!(
        "closed WAL: sidecars initially absent; after report wal={:?}, shm={:?}; main database bytes unchanged",
        sidecar_state(&wal),
        sidecar_state(&shm)
    );
    let reader = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("inspect fixture without a writable connection");
    let integrity: String = reader
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .expect("check closed WAL database integrity");
    let mode: String = reader
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("check persistent journal mode");
    let count: i64 = reader
        .query_row("SELECT COUNT(*) FROM sir_audit", [], |row| row.get(0))
        .expect("check retained finding");
    assert_eq!(integrity, "ok");
    assert_eq!(mode, "wal");
    assert_eq!(count, 1);
}
