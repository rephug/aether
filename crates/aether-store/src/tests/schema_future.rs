use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

fn schema_rows(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master ORDER BY name")
        .expect("schema")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("schema rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect schema")
}

#[test]
fn future_migration_fails_before_any_schema_data_or_receipt_write() {
    let conn = Connection::open_in_memory().expect("connection");
    run_migrations(&conn).expect("current schema");
    conn.execute_batch("CREATE TABLE future_sentinel (value TEXT); INSERT INTO future_sentinel VALUES ('keep'); DROP INDEX idx_symbols_qualified_name; PRAGMA user_version = 999; UPDATE schema_version SET version = 999, migrated_at = 123 WHERE component = 'core';").expect("future fixture");
    let schema_before = schema_rows(&conn);
    let changes_before: i64 = conn
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .expect("changes");
    let error = run_migrations(&conn).expect_err("future schema rejected");
    assert!(
        error
            .to_string()
            .contains("unsupported core schema version 999")
    );
    assert_eq!(schema_rows(&conn), schema_before);
    assert_eq!(
        conn.query_row("SELECT total_changes()", [], |row| row.get::<_, i64>(0))
            .expect("changes"),
        changes_before
    );
    assert_eq!(
        conn.query_row("SELECT value FROM future_sentinel", [], |row| row
            .get::<_, String>(0))
            .expect("sentinel"),
        "keep"
    );
    assert_eq!(
        conn.query_row(
            "SELECT version, migrated_at FROM schema_version WHERE component = 'core'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        )
        .expect("receipt"),
        (999, 123)
    );
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i32>(0))
            .expect("version"),
        999
    );
}

#[test]
fn invalid_negative_schema_is_not_treated_as_an_empty_database() {
    let conn = Connection::open_in_memory().expect("connection");
    conn.execute_batch("CREATE TABLE sentinel (value INTEGER); INSERT INTO sentinel VALUES (7); PRAGMA user_version = -1;").expect("negative fixture");
    let schema_before = schema_rows(&conn);
    let error = run_migrations(&conn).expect_err("negative version");
    assert!(
        error
            .to_string()
            .contains("unsupported core schema version -1")
    );
    assert_eq!(schema_rows(&conn), schema_before);
    assert_eq!(
        conn.query_row("SELECT value FROM sentinel", [], |row| row.get::<_, i64>(0))
            .expect("sentinel"),
        7
    );
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).expect("fixture inventory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            let relative = path.strip_prefix(root).expect("relative").to_path_buf();
            if path.is_dir() {
                output.insert(relative, None);
                walk(root, &path, output);
            } else {
                output.insert(relative, Some(fs::read(path).expect("file bytes")));
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    result
}

#[test]
fn future_store_openers_preserve_files_and_refuse_unknown_shapes() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("current store");
    store.upsert_symbol(symbol_record()).expect("known record");
    drop(store);
    let conn =
        Connection::open(temp.path().join(".aether/meta.sqlite")).expect("fixture connection");
    // A stable DELETE-journal fixture makes byte/inventory comparison exact.
    // Ordinary WAL readers may still require SQLite coordination files; this is
    // not a universal filesystem-immutability claim against a live WAL writer.
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA user_version = 999; UPDATE schema_version SET version = 999 WHERE component = 'core';").expect("future fixture");
    drop(conn);
    fs::remove_dir(temp.path().join(".aether/sir"))
        .expect("remove absent future artifact directory");
    let before = inventory(temp.path());
    let readonly = match SqliteStore::open_readonly(temp.path()) {
        Ok(_) => panic!("read-only future version was accepted"),
        Err(error) => error,
    };
    assert!(
        readonly
            .to_string()
            .contains("unsupported core schema version 999")
    );
    assert_eq!(
        inventory(temp.path()),
        before,
        "read-only rejection created or changed a file"
    );
    let writable = match SqliteStore::open(temp.path()) {
        Ok(_) => panic!("writable future version was accepted"),
        Err(error) => error,
    };
    assert!(
        writable
            .to_string()
            .contains("unsupported core schema version 999")
    );
    assert_eq!(
        inventory(temp.path()),
        before,
        "writable rejection changed files or journal mode"
    );
}
