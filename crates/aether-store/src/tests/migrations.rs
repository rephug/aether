//! Concurrency and atomicity of `run_migrations`.

use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, Barrier, mpsc};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, Transaction, TransactionBehavior};
use tempfile::tempdir;

use super::*;
use crate::schema::apply_migrations;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CORE_VERSION: i32 = CORE_SCHEMA_VERSION as i32;
const PARALLEL_OPENERS: usize = 8;
const PARALLEL_ROUNDS: usize = 6;

thread_local! {
    static FIRST_BUSY: RefCell<Option<mpsc::Sender<()>>> = const { RefCell::new(None) };
}

/// Busy handler that reports the first time this connection waits for a lock,
/// then keeps retrying for up to about five seconds.
fn report_first_busy(retries: i32) -> bool {
    if retries == 0 {
        FIRST_BUSY.with(|slot| {
            if let Some(sender) = slot.borrow().as_ref() {
                let _ = sender.send(());
            }
        });
    }
    if retries >= 5000 {
        return false;
    }
    std::thread::sleep(Duration::from_millis(1));
    true
}

fn user_version(conn: &Connection) -> rusqlite::Result<i32> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn has_table(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        [table, column],
        |row| row.get(0),
    )
}

/// SQLite's schema cookie: it changes whenever any connection changes the schema.
fn schema_cookie(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA schema_version", [], |row| row.get(0))
}

fn assert_final_schema(conn: &Connection) -> TestResult {
    assert_eq!(user_version(conn)?, CORE_VERSION);
    let core: i32 = conn.query_row(
        "SELECT version FROM schema_version WHERE component = 'core'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(core, CORE_VERSION);
    for table in ["symbols", "sir", "sir_audit", "sir_fingerprint_history"] {
        assert!(has_table(conn, table)?, "missing table {table}");
    }
    // The three most recent migrations each add one column.
    for (table, column) in [
        ("sir", "write_generation"),
        ("sir_fingerprint_history", "sir_write_generation"),
        ("sir", "source_hash"),
    ] {
        assert!(
            has_column(conn, table, column)?,
            "missing column {table}.{column}"
        );
    }
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    assert_eq!(integrity, "ok");
    Ok(())
}

fn assert_workspace_schema(workspace: &Path) -> TestResult {
    let conn = Connection::open(workspace.join(".aether/meta.sqlite"))?;
    assert_final_schema(&conn)
}

#[test]
fn parallel_fresh_store_opens_all_reach_current_schema() -> TestResult {
    let temp = tempdir()?;
    for round in 0..PARALLEL_ROUNDS {
        let workspace = temp.path().join(format!("workspace-{round}"));
        let barrier = Arc::new(Barrier::new(PARALLEL_OPENERS));
        let openers = (0..PARALLEL_OPENERS)
            .map(|_| {
                let workspace = workspace.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || -> Result<u32, StoreError> {
                    barrier.wait();
                    Ok(SqliteStore::open(&workspace)?.get_schema_version()?.version)
                })
            })
            .collect::<Vec<_>>();
        for opener in openers {
            let version = opener.join().expect("store opener panicked")?;
            assert_eq!(version, CORE_VERSION as u32, "round {round}");
        }
        for _ in 0..3 {
            let store = SqliteStore::open(&workspace)?;
            assert_eq!(store.get_schema_version()?.version, CORE_VERSION as u32);
        }
        assert_workspace_schema(&workspace)?;
    }
    Ok(())
}

#[test]
fn current_schema_adds_missing_graph_lookup_indexes_without_rewriting_data() -> TestResult {
    let conn = Connection::open_in_memory()?;
    run_migrations(&conn)?;
    conn.execute_batch(
        "DROP INDEX idx_symbols_qualified_name;
         DROP INDEX idx_symbols_file_path;
         INSERT INTO symbols
             (id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at)
         VALUES ('kept', 'src/lib.rs', 'rust', 'function', 'demo::kept', 'sig-kept', 1);",
    )?;
    assert_eq!(user_version(&conn)?, CORE_VERSION);

    run_migrations(&conn)?;
    for (column, index) in [
        ("qualified_name", "idx_symbols_qualified_name"),
        ("file_path", "idx_symbols_file_path"),
    ] {
        let query = format!("EXPLAIN QUERY PLAN SELECT id FROM symbols WHERE {column} = ?1");
        let detail: String = conn.query_row(&query, ["demo::kept"], |row| row.get(3))?;
        assert!(detail.contains(index), "expected {index}, got {detail}");
    }
    let kept: String = conn.query_row(
        "SELECT qualified_name FROM symbols WHERE id = 'kept'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(kept, "demo::kept");
    let cookie = schema_cookie(&conn)?;
    run_migrations(&conn)?;
    assert_eq!(schema_cookie(&conn)?, cookie, "reopen changed the schema");
    assert_final_schema(&conn)
}

#[test]
fn graph_store_and_store_open_same_fresh_workspace_together() -> TestResult {
    let temp = tempdir()?;
    for round in 0..PARALLEL_ROUNDS {
        let workspace = temp.path().join(format!("workspace-{round}"));
        let barrier = Arc::new(Barrier::new(PARALLEL_OPENERS));
        let openers = (0..PARALLEL_OPENERS)
            .map(|index| {
                let workspace = workspace.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || -> Result<(), StoreError> {
                    barrier.wait();
                    if index % 2 == 0 {
                        SqliteGraphStore::open(&workspace).map(drop)
                    } else {
                        SqliteStore::open(&workspace).map(drop)
                    }
                })
            })
            .collect::<Vec<_>>();
        for opener in openers {
            opener.join().expect("store opener panicked")?;
        }
        SqliteGraphStore::open(&workspace)?;
        SqliteStore::open(&workspace)?;
        assert_workspace_schema(&workspace)?;
    }
    Ok(())
}

#[test]
fn migration_waiter_reads_version_only_after_winner_commits() -> TestResult {
    let temp = tempdir()?;
    let path = temp.path().join("meta.sqlite");
    let winner = Connection::open(&path)?;
    winner.pragma_update(None, "journal_mode", "WAL")?;
    let tx = Transaction::new_unchecked(&winner, TransactionBehavior::Immediate)?;

    let (sender, first_busy) = mpsc::channel();
    let waiter_path = path.clone();
    let waiter = std::thread::spawn(move || -> Result<(), StoreError> {
        let conn = Connection::open(waiter_path)?;
        FIRST_BUSY.with(|slot| *slot.borrow_mut() = Some(sender));
        conn.busy_handler(Some(report_first_busy))?;
        run_migrations(&conn)
    });

    // The waiter is now blocked on the winner's write lock.
    first_busy.recv_timeout(Duration::from_secs(10))?;
    let version = user_version(&tx)?;
    apply_migrations(&tx, version)?;
    tx.commit()?;
    let cookie_after_winner = schema_cookie(&winner)?;

    waiter.join().expect("migration waiter panicked")?;
    // A waiter that read the version before the winner committed would re-run
    // old steps (for example recreating `sir_quality`) and change the schema.
    assert_eq!(
        schema_cookie(&winner)?,
        cookie_after_winner,
        "waiter re-ran migrations from a stale version"
    );
    assert_final_schema(&winner)
}

#[test]
fn migration_busy_timeout_leaves_database_retryable() -> TestResult {
    let temp = tempdir()?;
    let path = temp.path().join("meta.sqlite");
    let writer = Connection::open(&path)?;
    let lock = Transaction::new_unchecked(&writer, TransactionBehavior::Immediate)?;

    let waiter = Connection::open(&path)?;
    waiter.busy_timeout(Duration::from_millis(50))?;
    let started = Instant::now();
    let result = run_migrations(&waiter);
    assert!(
        matches!(&result, Err(StoreError::Sqlite(err)) if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)),
        "expected SQLITE_BUSY, got {result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        waiter.is_autocommit(),
        "failed migration left a transaction open"
    );

    lock.rollback()?;
    assert_eq!(user_version(&waiter)?, 0);
    assert!(!has_table(&waiter, "symbols")?);
    run_migrations(&waiter)?;
    assert_final_schema(&waiter)
}

#[test]
fn migration_late_failure_rolls_back_whole_sequence() -> TestResult {
    let conn = Connection::open_in_memory()?;
    // A pre-existing `sir_audit` without the columns v18 indexes makes the
    // v18 step fail after every earlier step has run.
    conn.execute_batch(
        "CREATE TABLE unrelated(value TEXT);
         INSERT INTO unrelated VALUES ('preserved');
         CREATE TABLE sir_audit(blocked INTEGER);",
    )?;

    assert!(run_migrations(&conn).is_err());
    assert!(
        conn.is_autocommit(),
        "failed migration left a transaction open"
    );
    assert_eq!(user_version(&conn)?, 0, "a partial version was published");
    assert!(!has_table(&conn, "symbols")?);
    assert!(!has_table(&conn, "schema_version")?);
    let kept: String = conn.query_row("SELECT value FROM unrelated", [], |row| row.get(0))?;
    assert_eq!(kept, "preserved");

    conn.execute("DROP TABLE sir_audit", [])?;
    run_migrations(&conn)?;
    assert_final_schema(&conn)
}

#[test]
fn wal_switch_waits_out_a_reader_and_fails_cleanly_on_timeout() -> TestResult {
    let temp = tempdir()?;
    let path = temp.path().join("meta.sqlite");
    let reader = Connection::open(&path)?;
    reader.execute_batch(
        "CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES ('preserved');",
    )?;
    // An open read transaction blocks the switch out of rollback journaling.
    let read = Transaction::new_unchecked(&reader, TransactionBehavior::Deferred)?;
    let kept: String = read.query_row("SELECT value FROM unrelated", [], |row| row.get(0))?;
    assert_eq!(kept, "preserved");

    let opener = Connection::open(&path)?;
    let started = Instant::now();
    let result = configure_wal(&opener, Duration::from_millis(50));
    assert!(
        matches!(&result, Err(StoreError::Sqlite(err)) if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)),
        "expected SQLITE_BUSY, got {result:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "gave up before the deadline"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(opener.is_autocommit());
    let mode: String = opener.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    assert_eq!(mode, "delete");

    read.rollback()?;
    configure_wal(&opener, Duration::from_secs(1))?;
    let mode: String = opener.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    assert_eq!(mode, "wal");
    run_migrations(&opener)?;
    assert_final_schema(&opener)?;
    let kept: String = opener.query_row("SELECT value FROM unrelated", [], |row| row.get(0))?;
    assert_eq!(kept, "preserved");
    Ok(())
}
